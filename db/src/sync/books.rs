//! Transactional orchestrator for the indexer write path. Owns
//! `sync_books`, the per-bucket helpers it composes
//! (`sync_removed` / `sync_changed` / `sync_new`), the
//! `replace_books` nuke-and-pave shim, and the per-book row writers
//! (`insert_book_row` / `update_book_row`) plus the metadata-link
//! dispatcher, FTS row insert, and post-commit cover materialization.

use sqlx::{SqlitePool, Transaction};

use omnibus_shared::EbookMetadata;

use crate::covers::{delete_cover_files_for, write_cover_file};
use crate::helpers::{parse_series_index, sanitize_accent_color, split_filename, stable_uuid};
use crate::settings::upsert_library;
use crate::taxonomy::{
    resolve_or_insert_language, resolve_or_insert_publisher, resolve_or_insert_series,
};

use super::authors::insert_author_links;
use super::backfill::backfill_stat_chunks;

/// Errors returned by the public sync write path.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<crate::settings::SettingsError> for SyncError {
    fn from(e: crate::settings::SettingsError) -> Self {
        match e {
            crate::settings::SettingsError::Db(inner) => SyncError::Db(inner),
        }
    }
}

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
) -> Result<(), SyncError> {
    let mut tx = pool.begin().await?;
    let library_id = upsert_library(&mut tx, library_path).await?;

    sync_removed(&mut tx, library_id, &plan.removed_uuids).await?;
    let changed_covers =
        sync_changed(&mut tx, library_id, library_path, &plan.changed_books).await?;
    let new_covers = sync_new(&mut tx, library_id, library_path, &plan.new_books).await?;
    backfill_stat_chunks(&mut tx, library_id, &plan.backfill).await?;
    stamp_last_indexed(&mut tx, library_id).await?;

    tx.commit().await?;

    // DB commit succeeded — reconcile the covers directory. All three steps
    // are synchronous `std::fs` (unlink orphans, write new/changed covers)
    // and scale with the diff size — thousands of files on a fresh library —
    // so run them together on the blocking pool rather than pinning a tokio
    // worker. A `JoinError` (panic in the reconcile) is logged and swallowed:
    // covers are a rebuildable cache, so a failed reconcile must not fail the
    // committed sync.
    let removed_uuids = plan.removed_uuids;
    if let Err(join_err) = tokio::task::spawn_blocking(move || {
        delete_cover_files_for(&removed_uuids);
        materialize_new_covers(new_covers);
        materialize_new_covers(changed_covers);
    })
    .await
    {
        tracing::error!("sync_books: cover reconcile spawn_blocking failed: {join_err}");
    }

    Ok(())
}

/// Remove a batch of books by uuid: explicit `books_fts` clear (FTS5 is
/// standalone — no FK cascade), then cascade DELETE from `books`.
async fn sync_removed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    removed_uuids: &[String],
) -> Result<(), sqlx::Error> {
    if removed_uuids.is_empty() {
        return Ok(());
    }
    // library_id + 1 bind per uuid; chunk at 500 to stay under SQLite's
    // 999-param cap when a whole library (or any large diff) is removed.
    // Both deletes run per chunk inside the same transaction, matching the
    // tags/identifiers/backfill chunking pattern elsewhere in this module.
    for chunk in removed_uuids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");

        // FTS5 is standalone (no FK to `books`), so we must clear it
        // explicitly before the cascade DELETE on `books` runs.
        let fts_sql = format!(
            "DELETE FROM books_fts WHERE rowid IN
                (SELECT id FROM books WHERE library_id = ? AND uuid IN ({placeholders}))"
        );
        let mut q = sqlx::query(&fts_sql).bind(library_id);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        q.execute(&mut **tx).await?;

        let books_sql =
            format!("DELETE FROM books WHERE library_id = ? AND uuid IN ({placeholders})");
        let mut q = sqlx::query(&books_sql).bind(library_id);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Apply Changed entries: wipe-and-rewrite the per-book link rows for each,
/// UPDATE the `books` row in place (preserving id), and re-insert the FTS
/// row. Returns `(uuid, mime, bytes)` triples for the post-commit cover
/// materialization.
///
/// If the diff said this uuid existed but a concurrent process removed it
/// between Phase A and the write, fall back to inserting it as a new book
/// rather than failing the whole sync over a TOCTOU.
async fn sync_changed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    library_path: &str,
    changed_books: &[crate::ebook::IndexedBook],
) -> Result<Vec<(String, String, Vec<u8>)>, sqlx::Error> {
    // Wipe-and-rewrite the per-book link rows for each Changed entry,
    // then UPDATE the `books` row and re-insert FTS. This trades two
    // small per-book deletes for the much simpler "compute the link
    // diff" alternative, while preserving `books.id` — which is the
    // only invariant any external caller depends on.
    let mut changed_covers: Vec<(String, String, Vec<u8>)> = Vec::new();
    for b in changed_books {
        let uuid = stable_uuid(library_path, &b.metadata.filename);
        let Some(book_id) =
            sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE library_id = ? AND uuid = ?")
                .bind(library_id)
                .bind(&uuid)
                .fetch_optional(&mut **tx)
                .await?
        else {
            // The diff said this uuid existed in the DB, but a concurrent
            // process removed it between Phase A and the write. Promote
            // to a New insert so the file still gets indexed; cleaner
            // than failing the whole sync over a TOCTOU.
            let inserted = insert_book_row(tx, library_id, library_path, b).await?;
            insert_metadata_links(tx, inserted.book_id, &b.metadata).await?;
            insert_fts_row(
                tx,
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

        update_book_row(tx, book_id, b).await?;
        wipe_per_book_link_rows(tx, book_id).await?;
        insert_book_file_row(tx, book_id, b).await?;
        insert_metadata_links(tx, book_id, &b.metadata).await?;
        // FTS5 row is keyed by rowid = book_id; delete + re-insert.
        sqlx::query("DELETE FROM books_fts WHERE rowid = ?")
            .bind(book_id)
            .execute(&mut **tx)
            .await?;
        let m = &b.metadata;
        let title = m.title.clone().unwrap_or_else(|| m.filename.clone());
        let first_isbn = first_isbn(&m.identifiers);
        insert_fts_row(tx, book_id, &title, first_isbn.as_deref(), &b.metadata).await?;

        if let Some((mime, bytes)) = &b.cover {
            changed_covers.push((uuid, mime.clone(), bytes.clone()));
        }
    }
    Ok(changed_covers)
}

/// Insert a batch of New entries: canonical `books` + `book_files` row,
/// metadata link rows, FTS row. Returns the post-commit cover triples.
async fn sync_new(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    library_path: &str,
    new_books: &[crate::ebook::IndexedBook],
) -> Result<Vec<(String, String, Vec<u8>)>, sqlx::Error> {
    let mut new_covers: Vec<(String, String, Vec<u8>)> = Vec::new();
    for b in new_books {
        let inserted = insert_book_row(tx, library_id, library_path, b).await?;
        insert_metadata_links(tx, inserted.book_id, &b.metadata).await?;
        insert_fts_row(
            tx,
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
    Ok(new_covers)
}

/// Stamp `libraries.last_indexed` with the current wall-clock seconds.
async fn stamp_last_indexed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
) -> Result<(), sqlx::Error> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    sqlx::query("UPDATE libraries SET last_indexed = ? WHERE id = ?")
        .bind(now)
        .bind(library_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Delete every per-book link row for `book_id`. Used by `sync_changed`
/// before re-inserting fresh rows — cascade delete on FK isn't an option
/// here (the `books` row stays), so we wipe explicitly. All these tables
/// have UNIQUE(book, ...) constraints, so a re-insert without the wipe
/// would fail.
async fn wipe_per_book_link_rows(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
) -> Result<(), sqlx::Error> {
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
        sqlx::query(&sql).bind(book_id).execute(&mut **tx).await?;
    }
    Ok(())
}

/// Insert the `book_files` row for an existing `books.id`. Shared by the
/// Changed re-insert path; the New path calls this indirectly via
/// `insert_book_row`.
async fn insert_book_file_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    b: &crate::ebook::IndexedBook,
) -> Result<(), sqlx::Error> {
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
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// First ISBN-scheme identifier, case-insensitive on the scheme name.
fn first_isbn(idents: &[omnibus_shared::Identifier]) -> Option<String> {
    idents
        .iter()
        .find(|id| {
            id.scheme
                .as_deref()
                .is_some_and(|s| s.eq_ignore_ascii_case("isbn"))
        })
        .map(|id| id.value.clone())
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
    let first_isbn = first_isbn(&m.identifiers);
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
) -> Result<(), SyncError> {
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
pub(super) struct InsertedBook {
    pub(super) book_id: i64,
    pub(super) uuid: String,
    pub(super) title: String,
    pub(super) first_isbn: Option<String>,
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
    let first_isbn = first_isbn(&m.identifiers);
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
///
/// The multi-valued relations (authors, tags, identifiers) are written in
/// batches rather than one statement per term: collect the distinct terms
/// for this book, `INSERT OR IGNORE` them all in one statement, then write
/// the join rows with one `INSERT ... SELECT ... JOIN` that resolves each id
/// inline by name (NOCASE) — no separate id-resolution `SELECT`. Each batched
/// statement is chunked to stay under SQLite's bound-parameter limit. This
/// collapses the old ~4-queries-per-author / 2-queries-per-tag fan-out into a
/// constant handful per book, which keeps the SQLite write lock from being
/// held for the whole of a bulk import. Series / publisher / language are
/// single-valued per book, so they keep the simple resolve-then-link path.
async fn insert_metadata_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    m: &EbookMetadata,
) -> Result<(), sqlx::Error> {
    insert_author_links(tx, book_id, m).await?;

    if let Some(series_name) = m.series.as_deref().filter(|s| !s.is_empty()) {
        let series_id = resolve_or_insert_series(tx, series_name).await?;
        sqlx::query("INSERT OR IGNORE INTO books_series_link (book, series) VALUES (?, ?)")
            .bind(book_id)
            .bind(series_id)
            .execute(&mut **tx)
            .await?;
    }

    insert_tag_links(tx, book_id, m).await?;

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

    insert_identifier_links(tx, book_id, m).await?;

    Ok(())
}

/// Batch-insert the book's tag (subject) join rows: one `INSERT OR IGNORE`
/// into `tags` for all distinct non-empty subjects, then one link insert that
/// resolves ids via a NOCASE join.
async fn insert_tag_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    m: &EbookMetadata,
) -> Result<(), sqlx::Error> {
    let mut seen = std::collections::HashSet::new();
    let tags: Vec<&str> = m
        .subjects
        .iter()
        .map(|s| s.as_str())
        .filter(|s| !s.is_empty())
        .filter(|s| seen.insert(*s))
        .collect();
    if tags.is_empty() {
        return Ok(());
    }
    // Both statements bind ~1 param per tag; chunk so a tag-heavy book can't
    // exceed SQLite's bound-parameter cap (999 by default). 500 keeps the link
    // statement (book_id + one per tag) safely under the limit.
    for chunk in tags.chunks(500) {
        let rows = std::iter::repeat_n("(?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let insert_sql = format!("INSERT OR IGNORE INTO tags (name) VALUES {rows}");
        let mut insert_q = sqlx::query(&insert_sql);
        for t in chunk {
            insert_q = insert_q.bind(*t);
        }
        insert_q.execute(&mut **tx).await?;

        let link_sql = format!(
            "INSERT OR IGNORE INTO books_tags_link (book, tag) \
             SELECT ?, t.id FROM (VALUES {rows}) AS v JOIN tags t ON t.name = v.column1"
        );
        let mut link_q = sqlx::query(&link_sql).bind(book_id);
        for t in chunk {
            link_q = link_q.bind(*t);
        }
        link_q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Batch-insert the book's identifiers in one statement. `INSERT OR REPLACE`
/// keeps the last value per `(book_id, scheme)`, matching the old loop.
async fn insert_identifier_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    m: &EbookMetadata,
) -> Result<(), sqlx::Error> {
    let idents: Vec<(&str, &str)> = m
        .identifiers
        .iter()
        .filter(|i| !i.value.is_empty())
        .map(|i| (i.scheme.as_deref().unwrap_or("unknown"), i.value.as_str()))
        .collect();
    if idents.is_empty() {
        return Ok(());
    }
    // 3 bound params per identifier; chunk at 250 (→ 750 binds) to stay under
    // SQLite's 999-param cap for books with very large identifier lists.
    // OR REPLACE keeps the last value per (book_id, scheme); processing chunks
    // in OPF order preserves that "last wins" semantics across chunk boundaries.
    for chunk in idents.chunks(250) {
        let rows = std::iter::repeat_n("(?, ?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT OR REPLACE INTO book_identifiers (book_id, scheme, value) VALUES {rows}"
        );
        let mut q = sqlx::query(&sql);
        for (scheme, value) in chunk {
            q = q.bind(book_id).bind(*scheme).bind(*value);
        }
        q.execute(&mut **tx).await?;
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
    let authors_text = crate::helpers::join_names(m.creators.iter().map(|c| c.name.as_str()));
    let series_text = m.series.clone().unwrap_or_default();
    let tags_text = crate::helpers::join_names(m.subjects.iter().map(String::as_str));
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

/// Write the cover bytes that accompanied a successful sync transaction.
/// Filesystem side-effect, so deliberately split out of the transactional
/// path — failures are logged, not fatal.
pub(super) fn materialize_new_covers(new_covers: Vec<(String, String, Vec<u8>)>) {
    for (uuid, mime, bytes) in new_covers {
        if let Err(e) = write_cover_file(&uuid, &mime, &bytes) {
            tracing::error!(
                error = %e,
                uuid = %uuid,
                "sync_books: failed to write cover"
            );
        }
    }
}
