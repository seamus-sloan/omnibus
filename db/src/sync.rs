//! Indexer write path. `sync_books` applies a per-bucket diff
//! (New / Changed / Removed / Backfill) atomically, then reconciles the
//! covers directory post-commit. `replace_books` is the nuke-and-pave
//! compatibility shim that the test suite drives.

use sqlx::{SqlitePool, Transaction};

use omnibus_shared::EbookMetadata;

use crate::covers::{delete_cover_files_for, write_cover_file};
use crate::helpers::{
    join_names, parse_series_index, sanitize_accent_color, split_filename, stable_uuid,
};
use crate::settings::upsert_library;
use crate::taxonomy::{
    resolve_or_insert_author, resolve_or_insert_language, resolve_or_insert_publisher,
    resolve_or_insert_series, resolve_or_insert_tag,
};

/// Per-bucket payload for [`sync_books`]. Built by
/// `crate::indexer::diff_library` (plus the Phase-B parse for new + changed).
///
/// The four buckets are mutually exclusive — a given uuid appears in at
/// most one of them per sync — and the diff already ordered them
/// deterministically.
#[derive(Debug, Default)]
pub struct SyncPlan {
    pub new_books: Vec<crate::ebook::IndexedBook>,
    pub changed_books: Vec<crate::ebook::IndexedBook>,
    pub removed_uuids: Vec<String>,
    /// `(uuid, mtime_epoch, size_bytes)` — see the Backfill section of
    /// the [`crate::indexer`] module doc for why this exists.
    pub backfill: Vec<(String, i64, i64)>,
}

/// Apply a per-bucket sync plan atomically. Unchanged books are not in
/// the plan, so their `books.id` is preserved by definition; Changed
/// books are UPDATEd in place so their `books.id` is preserved too.
///
/// Inside a single transaction, in this order:
/// 1. Upsert the `libraries` row.
/// 2. Delete Removed: explicit FTS clear + cascade DELETE from `books`.
/// 3. Update Changed in place (preserves `books.id`); wipe-and-rewrite
///    link rows + FTS row for each.
/// 4. Insert New (autoincrement assigns a fresh id).
/// 5. Backfill: UPDATE `book_files.(mtime_epoch, size_bytes)` only — no
///    OPF re-parse, no link writes, no FTS write. See the Backfill rule
///    in the [`crate::indexer`] module doc.
/// 6. Stamp `libraries.last_indexed`.
///
/// Post-commit (best-effort, logged on failure — covers are a
/// rebuildable cache):
/// - Delete cover files for Removed uuids only.
/// - Write cover files for New + Changed (overwrites the old file in
///   place; mime change sweeps the stale-extension orphan).
///
/// `metadata_overrides` is intentionally not touched — keyed by
/// `book_uuid` with no FK to `books.id` (see `0007_metadata_overrides.sql`).
/// User edits survive Changed UPDATEs and even Removed→New cycles for
/// the same filename.
pub async fn sync_books(
    pool: &SqlitePool,
    library_path: &str,
    plan: SyncPlan,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let library_id = upsert_library(&mut tx, library_path).await?;

    // --- Removed ---------------------------------------------------------
    if !plan.removed_uuids.is_empty() {
        let placeholders = std::iter::repeat_n("?", plan.removed_uuids.len())
            .collect::<Vec<_>>()
            .join(", ");

        // FTS5 is standalone (no FK to `books`), so we must clear it
        // explicitly before the cascade DELETE on `books` runs.
        let fts_sql = format!(
            "DELETE FROM books_fts WHERE rowid IN
                (SELECT id FROM books WHERE library_id = ? AND uuid IN ({placeholders}))"
        );
        let mut q = sqlx::query(&fts_sql).bind(library_id);
        for uuid in &plan.removed_uuids {
            q = q.bind(uuid);
        }
        q.execute(&mut *tx).await?;

        let books_sql =
            format!("DELETE FROM books WHERE library_id = ? AND uuid IN ({placeholders})");
        let mut q = sqlx::query(&books_sql).bind(library_id);
        for uuid in &plan.removed_uuids {
            q = q.bind(uuid);
        }
        q.execute(&mut *tx).await?;
    }

    // --- Changed ---------------------------------------------------------
    //
    // Wipe-and-rewrite the per-book link rows for each Changed entry,
    // then UPDATE the `books` row and re-insert FTS. This trades two
    // small per-book deletes for the much simpler "compute the link
    // diff" alternative, while preserving `books.id` — which is the
    // only invariant any external caller depends on.
    let mut changed_covers: Vec<(String, String, Vec<u8>)> = Vec::new();
    for b in &plan.changed_books {
        let uuid = stable_uuid(library_path, &b.metadata.filename);
        let Some(book_id) =
            sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE library_id = ? AND uuid = ?")
                .bind(library_id)
                .bind(&uuid)
                .fetch_optional(&mut *tx)
                .await?
        else {
            // The diff said this uuid existed in the DB, but a concurrent
            // process removed it between Phase A and the write. Promote
            // to a New insert so the file still gets indexed; cleaner
            // than failing the whole sync over a TOCTOU.
            let inserted = insert_book_row(&mut tx, library_id, library_path, b).await?;
            insert_metadata_links(&mut tx, inserted.book_id, &b.metadata).await?;
            insert_fts_row(
                &mut tx,
                inserted.book_id,
                &inserted.title,
                inserted.first_isbn.as_deref(),
                &b.metadata,
            )
            .await?;
            if let Some((mime, bytes)) = &b.cover {
                changed_covers.push((inserted.uuid, mime.clone(), bytes.clone()));
            }
            continue;
        };

        update_book_row(&mut tx, book_id, b).await?;
        // Cascade delete on FK isn't an option here (the `books` row
        // stays), so wipe the per-book join rows explicitly. All these
        // tables have UNIQUE(book, ...) constraints, so a re-insert
        // without the wipe would fail.
        for table in &[
            "book_files",
            "book_identifiers",
            "books_authors_link",
            "books_tags_link",
            "books_publishers_link",
            "books_series_link",
            "books_languages_link",
        ] {
            // Note: the link tables use `book` (not `book_id`) as the
            // FK column, but `book_files` and `book_identifiers` use
            // `book_id`. Switch on the table name.
            let col = if *table == "book_files" || *table == "book_identifiers" {
                "book_id"
            } else {
                "book"
            };
            let sql = format!("DELETE FROM {table} WHERE {col} = ?");
            sqlx::query(&sql).bind(book_id).execute(&mut *tx).await?;
        }
        // Re-insert the book_files row with the fresh fs metadata. The
        // INSERT body matches `insert_book_row` exactly.
        let m = &b.metadata;
        let (_, file_stem, file_ext) = split_filename(&m.filename);
        let mtime = m.modified.clone().unwrap_or_default();
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime, mtime_epoch)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(book_id)
        .bind(&file_ext)
        .bind(&file_stem)
        .bind(b.size_bytes)
        .bind(&mtime)
        .bind(b.mtime_epoch)
        .execute(&mut *tx)
        .await?;
        insert_metadata_links(&mut tx, book_id, &b.metadata).await?;
        // FTS5 row is keyed by rowid = book_id; delete + re-insert.
        sqlx::query("DELETE FROM books_fts WHERE rowid = ?")
            .bind(book_id)
            .execute(&mut *tx)
            .await?;
        let title = m.title.clone().unwrap_or_else(|| m.filename.clone());
        let first_isbn = m
            .identifiers
            .iter()
            .find(|id| {
                id.scheme
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case("isbn"))
            })
            .map(|id| id.value.clone());
        insert_fts_row(&mut tx, book_id, &title, first_isbn.as_deref(), &b.metadata).await?;

        if let Some((mime, bytes)) = &b.cover {
            changed_covers.push((uuid, mime.clone(), bytes.clone()));
        }
    }

    // --- New -------------------------------------------------------------
    let mut new_covers: Vec<(String, String, Vec<u8>)> = Vec::new();
    for b in &plan.new_books {
        let inserted = insert_book_row(&mut tx, library_id, library_path, b).await?;
        insert_metadata_links(&mut tx, inserted.book_id, &b.metadata).await?;
        insert_fts_row(
            &mut tx,
            inserted.book_id,
            &inserted.title,
            inserted.first_isbn.as_deref(),
            &b.metadata,
        )
        .await?;
        if let Some((mime, bytes)) = &b.cover {
            new_covers.push((inserted.uuid, mime.clone(), bytes.clone()));
        }
    }

    // --- Backfill --------------------------------------------------------
    for (uuid, mtime_epoch, size_bytes) in &plan.backfill {
        sqlx::query(
            "UPDATE book_files SET mtime_epoch = ?, size_bytes = ?
             WHERE book_id = (SELECT id FROM books WHERE library_id = ? AND uuid = ?)",
        )
        .bind(mtime_epoch)
        .bind(size_bytes)
        .bind(library_id)
        .bind(uuid)
        .execute(&mut *tx)
        .await?;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    sqlx::query("UPDATE libraries SET last_indexed = ? WHERE id = ?")
        .bind(now)
        .bind(library_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    // DB commit succeeded — reconcile the covers directory.
    delete_cover_files_for(&plan.removed_uuids);
    materialize_new_covers(new_covers);
    materialize_new_covers(changed_covers);

    Ok(())
}

/// UPDATE the `books` row for a Changed entry in place (preserving id).
/// All scalar columns that `insert_book_row` writes get refreshed; the
/// link tables and FTS row are handled by the caller.
async fn update_book_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    b: &crate::ebook::IndexedBook,
) -> Result<(), sqlx::Error> {
    let m = &b.metadata;
    let (book_path, _, _) = split_filename(&m.filename);
    let title = m.title.clone().unwrap_or_else(|| m.filename.clone());
    let series_index_num = m.series_index.as_deref().and_then(parse_series_index);
    let author_sort = m
        .creators
        .first()
        .and_then(|c| c.file_as.clone())
        .or_else(|| m.creators.first().map(|c| c.name.clone()));
    let first_isbn = m
        .identifiers
        .iter()
        .find(|id| {
            id.scheme
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("isbn"))
        })
        .map(|id| id.value.clone());
    let has_cover = i64::from(b.cover.is_some());

    sqlx::query(
        "UPDATE books SET
            path = ?, title = ?, sort = ?, author_sort = ?, series_index = ?,
            pubdate = ?, has_cover = ?, description = ?, isbn = ?, accent_color = ?,
            last_modified = datetime('now')
         WHERE id = ?",
    )
    .bind(&book_path)
    .bind(&title)
    .bind(&title)
    .bind(&author_sort)
    .bind(series_index_num)
    .bind(&m.published)
    .bind(has_cover)
    .bind(&m.description)
    .bind(&first_isbn)
    .bind(sanitize_accent_color(m.accent.as_deref()))
    .bind(book_id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Atomically replace every book under `library_path` with `books` and stamp
/// the last-indexed time. Thin compatibility shim over [`sync_books`]: it
/// computes the diff implicitly by treating every existing book as
/// Removed and every passed-in book as New. Kept for tests and any
/// caller that still wants the nuke-and-pave semantics; production
/// reindex goes through [`sync_books`] directly via
/// `crate::indexer::reindex`.
pub async fn replace_books(
    pool: &SqlitePool,
    library_path: &str,
    books: Vec<crate::ebook::IndexedBook>,
) -> Result<(), sqlx::Error> {
    let removed_uuids: Vec<String> = sqlx::query_scalar(
        "SELECT b.uuid FROM books b
         JOIN libraries l ON l.id = b.library_id
         WHERE l.path = ?",
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;

    sync_books(
        pool,
        library_path,
        SyncPlan {
            new_books: books,
            changed_books: vec![],
            removed_uuids,
            backfill: vec![],
        },
    )
    .await
}

/// Fields the per-book outer loop needs after the canonical `books` /
/// `book_files` inserts have run.
struct InsertedBook {
    book_id: i64,
    uuid: String,
    title: String,
    first_isbn: Option<String>,
}

/// Insert the canonical `books` row (returning its id) and its single
/// `book_files` row.
async fn insert_book_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    library_path: &str,
    b: &crate::ebook::IndexedBook,
) -> Result<InsertedBook, sqlx::Error> {
    let m = &b.metadata;
    let uuid = stable_uuid(library_path, &m.filename);
    let (book_path, file_stem, file_ext) = split_filename(&m.filename);
    let title = m.title.clone().unwrap_or_else(|| m.filename.clone());
    let series_index_num = m.series_index.as_deref().and_then(parse_series_index);
    let author_sort = m
        .creators
        .first()
        .and_then(|c| c.file_as.clone())
        .or_else(|| m.creators.first().map(|c| c.name.clone()));
    let first_isbn = m
        .identifiers
        .iter()
        .find(|id| {
            id.scheme
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("isbn"))
        })
        .map(|id| id.value.clone());
    let has_cover = i64::from(b.cover.is_some());

    let book_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO books
            (uuid, library_id, path, title, sort, author_sort, series_index,
             pubdate, has_cover, description, isbn, accent_color)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         RETURNING id",
    )
    .bind(&uuid)
    .bind(library_id)
    .bind(&book_path)
    .bind(&title)
    .bind(&title)
    .bind(&author_sort)
    .bind(series_index_num)
    .bind(&m.published)
    .bind(has_cover)
    .bind(&m.description)
    .bind(&first_isbn)
    .bind(sanitize_accent_color(m.accent.as_deref()))
    .fetch_one(&mut **tx)
    .await?;

    // The legacy `mtime TEXT` column holds the OPF `dcterms:modified` value
    // (Dublin Core, not filesystem state) — kept for backward compat. The
    // new `mtime_epoch INTEGER` column holds the filesystem stat the
    // incremental diff compares against (migration 0009).
    let mtime = m.modified.clone().unwrap_or_default();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime, mtime_epoch)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(book_id)
    .bind(&file_ext)
    .bind(&file_stem)
    .bind(b.size_bytes)
    .bind(&mtime)
    .bind(b.mtime_epoch)
    .execute(&mut **tx)
    .await?;

    Ok(InsertedBook {
        book_id,
        uuid,
        title,
        first_isbn,
    })
}

/// Insert the per-book metadata join rows (authors + contributors, series,
/// tags, publisher, language, identifiers).
async fn insert_metadata_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    m: &EbookMetadata,
) -> Result<(), sqlx::Error> {
    // Authors + contributors both land in `authors` — role/file_as are
    // flattened. Positions follow the OPF's source order
    // (`creators.iter().enumerate()`) so the primary author stays
    // primary. Names matching the `ignored_authors` blocklist resolve
    // to `None` and are skipped entirely; we leave a gap in `position`
    // rather than renumbering, so a blocklisted leading contributor
    // can produce a row whose first link is at position 1 — the
    // surviving creators keep their original ordinal either way.
    for (pos, c) in m.creators.iter().enumerate() {
        let Some(author_id) = resolve_or_insert_author(tx, &c.name, c.file_as.as_deref()).await?
        else {
            continue;
        };
        sqlx::query(
            "INSERT OR IGNORE INTO books_authors_link (book, author, position)
             VALUES (?, ?, ?)",
        )
        .bind(book_id)
        .bind(author_id)
        .bind(pos as i64)
        .execute(&mut **tx)
        .await?;
    }
    let author_count = m.creators.len();
    for (i, c) in m.contributors.iter().enumerate() {
        let Some(author_id) = resolve_or_insert_author(tx, &c.name, c.file_as.as_deref()).await?
        else {
            continue;
        };
        sqlx::query(
            "INSERT OR IGNORE INTO books_authors_link (book, author, position)
             VALUES (?, ?, ?)",
        )
        .bind(book_id)
        .bind(author_id)
        .bind((author_count + i) as i64)
        .execute(&mut **tx)
        .await?;
    }

    if let Some(series_name) = m.series.as_deref().filter(|s| !s.is_empty()) {
        let series_id = resolve_or_insert_series(tx, series_name).await?;
        sqlx::query("INSERT OR IGNORE INTO books_series_link (book, series) VALUES (?, ?)")
            .bind(book_id)
            .bind(series_id)
            .execute(&mut **tx)
            .await?;
    }

    for subject in &m.subjects {
        if subject.is_empty() {
            continue;
        }
        let tag_id = resolve_or_insert_tag(tx, subject).await?;
        sqlx::query("INSERT OR IGNORE INTO books_tags_link (book, tag) VALUES (?, ?)")
            .bind(book_id)
            .bind(tag_id)
            .execute(&mut **tx)
            .await?;
    }

    if let Some(pub_name) = m.publisher.as_deref().filter(|s| !s.is_empty()) {
        let pub_id = resolve_or_insert_publisher(tx, pub_name).await?;
        sqlx::query("INSERT OR IGNORE INTO books_publishers_link (book, publisher) VALUES (?, ?)")
            .bind(book_id)
            .bind(pub_id)
            .execute(&mut **tx)
            .await?;
    }

    if let Some(lang_code) = m.language.as_deref().filter(|s| !s.is_empty()) {
        let lang_id = resolve_or_insert_language(tx, lang_code).await?;
        sqlx::query("INSERT OR IGNORE INTO books_languages_link (book, language) VALUES (?, ?)")
            .bind(book_id)
            .bind(lang_id)
            .execute(&mut **tx)
            .await?;
    }

    for ident in &m.identifiers {
        if ident.value.is_empty() {
            continue;
        }
        let scheme = ident
            .scheme
            .clone()
            .unwrap_or_else(|| "unknown".to_string());
        sqlx::query(
            "INSERT OR REPLACE INTO book_identifiers (book_id, scheme, value)
             VALUES (?, ?, ?)",
        )
        .bind(book_id)
        .bind(&scheme)
        .bind(&ident.value)
        .execute(&mut **tx)
        .await?;
    }

    Ok(())
}

/// Write the `books_fts` row for a book. Inline (rather than a trigger) so
/// the bulk reindex doesn't fan out across six tables; keeps the
/// denormalized row in lock-step with the canonical inserts.
pub(crate) async fn insert_fts_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    title: &str,
    first_isbn: Option<&str>,
    m: &EbookMetadata,
) -> Result<(), sqlx::Error> {
    let authors_text = join_names(
        m.creators
            .iter()
            .chain(m.contributors.iter())
            .map(|c| c.name.as_str()),
    );
    let series_text = m.series.clone().unwrap_or_default();
    let tags_text = join_names(m.subjects.iter().map(String::as_str));
    sqlx::query(
        "INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(book_id)
    .bind(title)
    .bind(&authors_text)
    .bind(&series_text)
    .bind(&tags_text)
    .bind(m.description.as_deref().unwrap_or(""))
    .bind(first_isbn.unwrap_or(""))
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Write the cover bytes that accompanied a successful `replace_books`
/// transaction. Filesystem side-effect, so deliberately split out of the
/// transactional path — failures are logged, not fatal.
fn materialize_new_covers(new_covers: Vec<(String, String, Vec<u8>)>) {
    for (uuid, mime, bytes) in new_covers {
        if let Err(e) = write_cover_file(&uuid, &mime, &bytes) {
            tracing::error!(
                error = %e,
                uuid = %uuid,
                "replace_books: failed to write cover"
            );
        }
    }
}
