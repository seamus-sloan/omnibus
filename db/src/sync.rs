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
    resolve_or_insert_language, resolve_or_insert_publisher, resolve_or_insert_series,
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
    // One `UPDATE ... FROM (VALUES ..)` per chunk instead of one statement per
    // book, so a post-migration run over a large library doesn't hold the
    // write lock for thousands of individual writes (issue #245). Chunked to
    // stay well under SQLite's bound-parameter limit (3 binds per row, plus 1
    // for library_id).
    for chunk in plan.backfill.chunks(250) {
        let rows = std::iter::repeat_n("(?, ?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE book_files SET mtime_epoch = v.column2, size_bytes = v.column3 \
             FROM (VALUES {rows}) AS v, books b \
             WHERE b.uuid = v.column1 AND b.library_id = ? AND book_files.book_id = b.id"
        );
        let mut q = sqlx::query(&sql);
        for (uuid, mtime_epoch, size_bytes) in chunk {
            q = q.bind(uuid).bind(mtime_epoch).bind(size_bytes);
        }
        q = q.bind(library_id);
        q.execute(&mut *tx).await?;
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
///
/// The multi-valued relations (authors, tags, identifiers) are written in
/// batches rather than one statement per term: collect the distinct terms
/// for this book, `INSERT OR IGNORE` them all in one statement, then write
/// the join rows with one `INSERT ... SELECT ... JOIN` that resolves each id
/// inline by name (NOCASE) — no separate id-resolution `SELECT`. Each batched
/// statement is chunked to stay under SQLite's bound-parameter limit. This
/// collapses the old ~4-queries-per-author / 2-queries-per-tag fan-out into a
/// constant handful per book, which keeps
/// the SQLite write lock from being held for the whole of a bulk import
/// (issue #242). Series / publisher / language are single-valued per book,
/// so they keep the simple resolve-then-link path.
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

/// Batch-insert author + contributor join rows. Creators take positions
/// `0..n`, contributors follow at `n..` (OPF source order). Names on the
/// `ignored_authors` blocklist are skipped — leaving a gap in `position`
/// rather than renumbering — identical to the previous per-author loop, but
/// resolved in a constant handful of statements instead of ~4 per author.
async fn insert_author_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    m: &EbookMetadata,
) -> Result<(), sqlx::Error> {
    // (name, sort, position) in OPF order: creators first, then contributors.
    let author_count = m.creators.len();
    let entries: Vec<(&str, Option<&str>, i64)> = m
        .creators
        .iter()
        .enumerate()
        .map(|(pos, c)| (c.name.as_str(), c.file_as.as_deref(), pos as i64))
        .chain(m.contributors.iter().enumerate().map(|(i, c)| {
            (
                c.name.as_str(),
                c.file_as.as_deref(),
                (author_count + i) as i64,
            )
        }))
        .collect();
    if entries.is_empty() {
        return Ok(());
    }

    // Distinct names (first-seen order), keeping the first non-null sort —
    // mirrors the old per-row `COALESCE(authors.sort, excluded.sort)`.
    let mut order: Vec<&str> = Vec::new();
    let mut sort_for: std::collections::HashMap<&str, Option<&str>> =
        std::collections::HashMap::new();
    for (name, sort, _) in &entries {
        if let Some(slot) = sort_for.get_mut(name) {
            if slot.is_none() {
                *slot = *sort;
            }
        } else {
            order.push(name);
            sort_for.insert(name, *sort);
        }
    }

    // Which of these names are blocklisted? Compared via `ignored_authors.name`
    // so SQLite applies the column's NOCASE collation (identical match
    // semantics to the old per-name `WHERE name = ?`). Returns the book's own
    // name strings (from the VALUES list), so the skip check below is exact.
    let name_rows = std::iter::repeat_n("(?)", order.len())
        .collect::<Vec<_>>()
        .join(", ");
    let ignored_sql = format!(
        "SELECT v.column1 FROM (VALUES {name_rows}) AS v \
         WHERE EXISTS (SELECT 1 FROM ignored_authors i WHERE i.name = v.column1)"
    );
    let mut ignored_q = sqlx::query_scalar::<_, String>(&ignored_sql);
    for name in &order {
        ignored_q = ignored_q.bind(*name);
    }
    let ignored: std::collections::HashSet<String> =
        ignored_q.fetch_all(&mut **tx).await?.into_iter().collect();

    let kept: Vec<&str> = order
        .iter()
        .copied()
        .filter(|n| !ignored.contains(*n))
        .collect();
    if kept.is_empty() {
        return Ok(());
    }

    // Upsert all kept authors in one statement.
    let author_rows = std::iter::repeat_n("(?, ?)", kept.len())
        .collect::<Vec<_>>()
        .join(", ");
    let upsert_sql = format!(
        "INSERT INTO authors (name, sort) VALUES {author_rows} \
         ON CONFLICT(name) DO UPDATE SET sort = COALESCE(authors.sort, excluded.sort)"
    );
    let mut upsert_q = sqlx::query(&upsert_sql);
    for name in &kept {
        upsert_q = upsert_q.bind(*name).bind(sort_for[*name]);
    }
    upsert_q.execute(&mut **tx).await?;

    // Link rows, deduped by name keeping the first (lowest) position so a name
    // appearing as both creator and contributor keeps its creator position —
    // matching the old `INSERT OR IGNORE` loop. The id is resolved in SQL via
    // the NOCASE join, so casing differences don't need handling in Rust.
    let mut linked: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let link_entries: Vec<(&str, i64)> = entries
        .iter()
        .filter(|(name, _, _)| !ignored.contains(*name))
        .filter(|(name, _, _)| linked.insert(name))
        .map(|(name, _, pos)| (*name, *pos))
        .collect();
    let link_rows = std::iter::repeat_n("(?, ?)", link_entries.len())
        .collect::<Vec<_>>()
        .join(", ");
    let link_sql = format!(
        "INSERT OR IGNORE INTO books_authors_link (book, author, position) \
         SELECT ?, a.id, v.column2 \
         FROM (VALUES {link_rows}) AS v \
         JOIN authors a ON a.name = v.column1"
    );
    let mut link_q = sqlx::query(&link_sql).bind(book_id);
    for (name, pos) in &link_entries {
        link_q = link_q.bind(*name).bind(*pos);
    }
    link_q.execute(&mut **tx).await?;
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

#[cfg(test)]
pub(crate) mod test_helpers {
    //! Shared `IndexedBook` builders used by db tests across modules.
    //! `pub(crate)` so siblings can call e.g.
    //! `use crate::sync::test_helpers::indexed;`.

    use crate::ebook::IndexedBook;
    use omnibus_shared::{Contributor, EbookMetadata};

    pub(crate) fn indexed(
        filename: &str,
        title: Option<&str>,
        authors: &[&str],
        subjects: &[&str],
        series: Option<(&str, &str)>,
        cover: Option<(&str, &[u8])>,
    ) -> IndexedBook {
        IndexedBook {
            metadata: EbookMetadata {
                filename: filename.into(),
                title: title.map(Into::into),
                creators: authors
                    .iter()
                    .map(|a| Contributor {
                        name: (*a).into(),
                        ..Default::default()
                    })
                    .collect(),
                subjects: subjects.iter().map(|s| (*s).to_string()).collect(),
                series: series.map(|(n, _)| n.into()),
                series_index: series.map(|(_, i)| i.into()),
                ..Default::default()
            },
            cover: cover.map(|(m, b)| (m.into(), b.to_vec())),
            mtime_epoch: 0,
            size_bytes: 0,
        }
    }
    // ------------------------------------------------------------------
    // sync_books — incremental write path. Each test seeds an initial
    // state via `replace_books` (the legacy nuke-and-pave wrapper), then
    // applies a hand-built `SyncPlan` to exercise one or more diff
    // buckets and asserts the post-state.
    // ------------------------------------------------------------------

    /// Build an `IndexedBook` matching `indexed(...)` but with the
    /// supplied (mtime_epoch, size_bytes). Used to drive the New +
    /// Changed branches of sync_books with realistic fs metadata.
    pub(crate) fn indexed_with_stat(
        filename: &str,
        title: Option<&str>,
        mtime_epoch: i64,
        size_bytes: i64,
    ) -> IndexedBook {
        IndexedBook {
            metadata: EbookMetadata {
                filename: filename.into(),
                title: title.map(Into::into),
                ..Default::default()
            },
            cover: None,
            mtime_epoch,
            size_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::test_helpers::{indexed, indexed_with_stat};
    use super::*;
    use crate::books::{get_book, list_books, list_indexed_rows, search_books};
    use crate::covers::get_cover;
    use crate::covers::test_helpers::CoversTempDir;
    use crate::ebook::IndexedBook;
    use crate::helpers::stable_uuid;
    use crate::metadata_overrides::upsert_metadata_overrides;
    use crate::pool::init_db;
    use crate::settings::last_indexed_at;
    use omnibus_shared::{Contributor, EbookMetadata, MetadataOverrides};

    #[tokio::test]
    async fn replace_books_inserts_metadata_and_covers() {
        let _covers = CoversTempDir::new("insert");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("A"),
                    &["Author A"],
                    &["fiction"],
                    Some(("Saga", "1")),
                    Some(("image/jpeg", b"BYTES")),
                ),
                indexed("b.epub", Some("B"), &["Author B"], &[], None, None),
            ],
        )
        .await
        .expect("replace should succeed");

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 2);

        let a = books
            .iter()
            .find(|b| b.title.as_deref() == Some("A"))
            .unwrap();
        let b = books
            .iter()
            .find(|b| b.title.as_deref() == Some("B"))
            .unwrap();

        assert_eq!(a.filename, "a.epub");
        assert_eq!(b.filename, "b.epub");
        assert_eq!(a.creators.len(), 1);
        assert_eq!(a.creators[0].name, "Author A");
        assert_eq!(a.subjects, vec!["fiction".to_string()]);
        assert_eq!(a.series.as_deref(), Some("Saga"));
        assert_eq!(a.series_index.as_deref(), Some("1"));

        let a_uuid = a.unique_identifier.clone().unwrap();
        assert_eq!(
            a.cover_url.as_deref(),
            Some(format!("/api/covers/{a_uuid}").as_str())
        );
        assert_eq!(b.cover_url, None);

        // F1.3: list_books exposes the row insertion timestamp so the
        // landing page can offer a "Newest Added" sort. The migration
        // defaults `books.timestamp` to `datetime('now')`
        // (`YYYY-MM-DD HH:MM:SS`, UTC), so every row surfaces a non-empty
        // sortable string.
        for book in &books {
            let added = book.added_at.as_deref().unwrap_or("");
            assert!(
                !added.is_empty(),
                "added_at should be populated for {:?}",
                book.title
            );
        }

        let cover = get_cover(&pool, a.id).await.unwrap();
        assert_eq!(cover, Some(("image/jpeg".into(), b"BYTES".to_vec())));
        assert!(get_cover(&pool, b.id).await.unwrap().is_none());

        assert!(last_indexed_at(&pool, "/lib").await.unwrap().is_some());
    }
    /// F1.7 Atrium accent round-trip. `replace_books` writes
    /// `metadata.accent` into `books.accent_color`; `list_books` /
    /// `get_book` / `search_books` read it back into
    /// `EbookMetadata.accent`. Verify the column survives the trip and
    /// `None` stays `None` (not coerced to an empty string).
    #[tokio::test]
    async fn replace_books_round_trips_accent_color() {
        let _covers = CoversTempDir::new("accent_round_trip");
        let pool = init_db("sqlite::memory:").await.unwrap();

        let with_accent = IndexedBook {
            metadata: EbookMetadata {
                filename: "with-accent.epub".into(),
                title: Some("Piranesi".into()),
                creators: vec![Contributor {
                    name: "Susanna Clarke".into(),
                    ..Default::default()
                }],
                accent: Some("oklch(0.660 0.130 245.0)".into()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
        };
        let no_accent = IndexedBook {
            metadata: EbookMetadata {
                filename: "no-accent.epub".into(),
                title: Some("Plain".into()),
                creators: vec![Contributor {
                    name: "Anon".into(),
                    ..Default::default()
                }],
                accent: None,
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
        };
        replace_books(&pool, "/lib", vec![with_accent, no_accent])
            .await
            .expect("replace should succeed");

        // list_books returns the accent column for every row.
        let books = list_books(&pool, "/lib").await.unwrap();
        let p = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Piranesi"))
            .unwrap();
        let plain = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Plain"))
            .unwrap();
        assert_eq!(p.accent.as_deref(), Some("oklch(0.660 0.130 245.0)"));
        assert_eq!(plain.accent, None);

        // get_book returns the same value through the single-row path.
        let detail = get_book(&pool, p.id).await.unwrap().unwrap();
        assert_eq!(detail.accent.as_deref(), Some("oklch(0.660 0.130 245.0)"));
        let detail_plain = get_book(&pool, plain.id).await.unwrap().unwrap();
        assert_eq!(detail_plain.accent, None);
    }
    /// #125: the write-boundary gate must accept the exact `oklch(L C H)`
    /// shape the indexer emits, and reject anything else — including raw
    /// hex, CSS keywords, and injection payloads that try to break out of
    /// the `style="background: {bg}"` attribute used by Atrium consumers.
    ///
    /// End-to-end gate: writing an `IndexedBook` whose `accent` carries an
    /// injection payload must result in `accent_color = NULL` in the DB,
    /// not the unsanitized string.
    #[tokio::test]
    async fn replace_books_drops_unsafe_accent_color() {
        let _covers = CoversTempDir::new("accent_unsafe");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let unsafe_book = IndexedBook {
            metadata: EbookMetadata {
                filename: "shady.epub".into(),
                title: Some("Shady".into()),
                creators: vec![Contributor {
                    name: "Anon".into(),
                    ..Default::default()
                }],
                accent: Some("red; background: url(x)".into()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
        };
        replace_books(&pool, "/lib", vec![unsafe_book])
            .await
            .expect("replace should succeed");
        let books = list_books(&pool, "/lib").await.unwrap();
        let shady = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Shady"))
            .unwrap();
        assert_eq!(shady.accent, None);
    }
    /// Seed two books via `replace_books`, then `sync_books` with no
    /// diff buckets at all. Both ids must survive — that's the whole
    /// point of the refactor.
    #[tokio::test]
    async fn sync_preserves_book_id_for_unchanged() {
        let _covers = CoversTempDir::new("sync_unchanged");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &[], &[], None, None),
                indexed("b.epub", Some("B"), &[], &[], None, None),
            ],
        )
        .await
        .unwrap();
        let before: Vec<_> = list_books(&pool, "/lib")
            .await
            .unwrap()
            .into_iter()
            .map(|b| (b.filename.clone(), b.id))
            .collect();

        sync_books(&pool, "/lib", SyncPlan::default())
            .await
            .unwrap();

        let after: Vec<_> = list_books(&pool, "/lib")
            .await
            .unwrap()
            .into_iter()
            .map(|b| (b.filename.clone(), b.id))
            .collect();
        assert_eq!(before, after, "ids must be preserved across a no-op sync");
    }
    #[tokio::test]
    async fn sync_preserves_book_id_for_changed() {
        let _covers = CoversTempDir::new("sync_changed");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Old Title"),
                &["Old Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let original_id = list_books(&pool, "/lib").await.unwrap()[0].id;

        // One Changed entry — same filename so same uuid, new title + author.
        let plan = SyncPlan {
            changed_books: vec![IndexedBook {
                metadata: EbookMetadata {
                    filename: "a.epub".into(),
                    title: Some("New Title".into()),
                    creators: vec![Contributor {
                        name: "New Author".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 999,
                size_bytes: 42,
            }],
            ..Default::default()
        };
        sync_books(&pool, "/lib", plan).await.unwrap();

        let after = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, original_id, "books.id must be preserved");
        assert_eq!(after[0].title.as_deref(), Some("New Title"));
        assert_eq!(after[0].creators.len(), 1);
        assert_eq!(after[0].creators[0].name, "New Author");
    }
    /// A user-supplied metadata override (keyed by `book_uuid`, no FK to
    /// `books.id`) must still apply after a Changed UPDATE — proving
    /// that the in-place UPDATE doesn't accidentally rotate the uuid
    /// and that the overrides table isn't touched by sync_books.
    #[tokio::test]
    async fn sync_overrides_survive_changed() {
        let _covers = CoversTempDir::new("sync_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("Scanned"), &[], &[], None, None)],
        )
        .await
        .unwrap();
        let book_uuid = list_indexed_rows(&pool, "/lib").await.unwrap()[0]
            .uuid
            .clone();

        // Write a user override that renames the title.
        let overrides = MetadataOverrides {
            title: Some("User Title".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &book_uuid, &overrides, false, user_id)
            .await
            .unwrap();

        // Now Change the book — the scan would happily say "Scanned"
        // again, but the override should still surface "User Title".
        let plan = SyncPlan {
            changed_books: vec![indexed_with_stat("a.epub", Some("Scanned v2"), 100, 100)],
            ..Default::default()
        };
        sync_books(&pool, "/lib", plan).await.unwrap();

        let after = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(after[0].title.as_deref(), Some("User Title"));
    }
    /// A Removed uuid must wipe books_fts, book_files,
    /// books_authors_link, etc. — the cascade plus our explicit FTS
    /// clear should leave no orphans.
    #[tokio::test]
    async fn sync_removes_book_cascades_links_and_fts() {
        let _covers = CoversTempDir::new("sync_removed_cascade");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "doomed.epub",
                Some("Doomed"),
                &["Anon"],
                &["fic"],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let book_id = list_books(&pool, "/lib").await.unwrap()[0].id;
        let uuid = list_indexed_rows(&pool, "/lib").await.unwrap()[0]
            .uuid
            .clone();

        let plan = SyncPlan {
            removed_uuids: vec![uuid],
            ..Default::default()
        };
        sync_books(&pool, "/lib", plan).await.unwrap();

        let books_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        let files_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM book_files WHERE book_id = ?")
                .bind(book_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let link_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM books_authors_link WHERE book = ?")
                .bind(book_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts WHERE rowid = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(books_count, 0);
        assert_eq!(files_count, 0);
        assert_eq!(link_count, 0);
        assert_eq!(fts_count, 0);
    }
    /// One sync covering all four mutating branches at once. Unchanged
    /// ids stay put; Changed id stays put; New gets a fresh id;
    /// Removed disappears.
    #[tokio::test]
    async fn sync_mixed_diff_in_one_transaction() {
        let _covers = CoversTempDir::new("sync_mixed");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("keep.epub", Some("Keep"), &[], &[], None, None),
                indexed("edit.epub", Some("Old Edit"), &[], &[], None, None),
                indexed("gone.epub", Some("Gone"), &[], &[], None, None),
            ],
        )
        .await
        .unwrap();
        let before: std::collections::HashMap<String, i64> = list_books(&pool, "/lib")
            .await
            .unwrap()
            .into_iter()
            .map(|b| (b.filename.clone(), b.id))
            .collect();
        let gone_uuid = stable_uuid("/lib", "gone.epub");

        let plan = SyncPlan {
            new_books: vec![indexed_with_stat("add.epub", Some("Added"), 100, 100)],
            changed_books: vec![indexed_with_stat("edit.epub", Some("New Edit"), 200, 200)],
            removed_uuids: vec![gone_uuid],
            backfill: vec![],
        };
        sync_books(&pool, "/lib", plan).await.unwrap();

        let after: std::collections::HashMap<String, i64> = list_books(&pool, "/lib")
            .await
            .unwrap()
            .into_iter()
            .map(|b| (b.filename.clone(), b.id))
            .collect();

        assert_eq!(after.len(), 3);
        assert_eq!(after.get("keep.epub"), before.get("keep.epub"));
        assert_eq!(after.get("edit.epub"), before.get("edit.epub"));
        assert!(after.contains_key("add.epub"));
        assert!(!after.contains_key("gone.epub"));
    }
    /// Removed books should lose their cover files; survivors' covers
    /// must stay intact. Catches "delete every cover on every sync"
    /// regressions if anyone ever short-circuits the bucket logic.
    #[tokio::test]
    async fn sync_cover_sidecar_lifecycle_on_remove() {
        let covers = CoversTempDir::new("sync_cover_remove");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "keep.epub",
                    Some("Keep"),
                    &[],
                    &[],
                    None,
                    Some(("image/jpeg", b"KEEP_BYTES")),
                ),
                indexed(
                    "gone.epub",
                    Some("Gone"),
                    &[],
                    &[],
                    None,
                    Some(("image/jpeg", b"GONE_BYTES")),
                ),
            ],
        )
        .await
        .unwrap();
        let keep_uuid = stable_uuid("/lib", "keep.epub");
        let gone_uuid = stable_uuid("/lib", "gone.epub");
        let keep_path = covers.path.join(format!("{keep_uuid}.jpg"));
        let gone_path = covers.path.join(format!("{gone_uuid}.jpg"));
        assert!(keep_path.exists(), "cover for keep should exist");
        assert!(gone_path.exists(), "cover for gone should exist");

        sync_books(
            &pool,
            "/lib",
            SyncPlan {
                removed_uuids: vec![gone_uuid],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert!(keep_path.exists(), "survivor cover must remain");
        assert!(!gone_path.exists(), "removed cover must be deleted");
    }
    /// FTS5 row carries `rowid = books.id`. After a Changed UPDATE the
    /// rowid must still equal the preserved id, and the index content
    /// must reflect the new title.
    #[tokio::test]
    async fn sync_fts_row_consistent_after_changed() {
        let _covers = CoversTempDir::new("sync_fts");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("Antarctica"), &[], &[], None, None)],
        )
        .await
        .unwrap();
        let original_id = list_books(&pool, "/lib").await.unwrap()[0].id;

        sync_books(
            &pool,
            "/lib",
            SyncPlan {
                changed_books: vec![indexed_with_stat("a.epub", Some("Borealis"), 200, 200)],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let stale = search_books(&pool, "/lib", "Antarctica").await.unwrap();
        let fresh = search_books(&pool, "/lib", "Borealis").await.unwrap();
        assert!(stale.is_empty(), "old title must not match after change");
        assert_eq!(fresh.len(), 1);
        assert_eq!(fresh[0].id, original_id, "FTS rowid stable across change");
    }
    /// The Backfill bucket fills in the post-migration sentinel stat
    /// values without touching any metadata columns. Confirm both
    /// invariants: stat populated, OPF-derived fields untouched.
    #[tokio::test]
    async fn sync_backfill_writes_stat_without_touching_metadata() {
        let _covers = CoversTempDir::new("sync_backfill");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("Original"), &[], &[], None, None)],
        )
        .await
        .unwrap();
        let uuid = list_indexed_rows(&pool, "/lib").await.unwrap()[0]
            .uuid
            .clone();
        // Confirm the row started at the (0, 0) sentinel — replace_books
        // wrote the IndexedBook stats (which the test fixture defaults
        // to 0).
        let pre = list_indexed_rows(&pool, "/lib").await.unwrap();
        assert_eq!(pre[0].mtime_epoch, 0);
        assert_eq!(pre[0].size_bytes, 0);

        sync_books(
            &pool,
            "/lib",
            SyncPlan {
                backfill: vec![(uuid, 1234, 5678)],
                ..Default::default()
            },
        )
        .await
        .unwrap();

        let post = list_indexed_rows(&pool, "/lib").await.unwrap();
        assert_eq!(post[0].mtime_epoch, 1234);
        assert_eq!(post[0].size_bytes, 5678);
        // Title is untouched — backfill must not have triggered any
        // metadata writes.
        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books[0].title.as_deref(), Some("Original"));
    }
    /// Empty disk → diff says "remove all" → sync_books wipes the
    /// library cleanly. Stress test for the Removed branch.
    #[tokio::test]
    async fn sync_empty_plan_with_full_removed_clears_library() {
        let _covers = CoversTempDir::new("sync_empty");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &[], &[], None, None),
                indexed("b.epub", Some("B"), &[], &[], None, None),
            ],
        )
        .await
        .unwrap();
        let all_uuids: Vec<String> = list_indexed_rows(&pool, "/lib")
            .await
            .unwrap()
            .into_iter()
            .map(|r| r.uuid)
            .collect();

        sync_books(
            &pool,
            "/lib",
            SyncPlan {
                removed_uuids: all_uuids,
                ..Default::default()
            },
        )
        .await
        .unwrap();

        assert!(list_books(&pool, "/lib").await.unwrap().is_empty());
    }
    #[tokio::test]
    async fn reindex_replaces_library_atomically() {
        let _covers = CoversTempDir::new("atomic");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Author A"],
                &["fiction"],
                None,
                Some(("image/jpeg", b"OLD")),
            )],
        )
        .await
        .unwrap();

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Author A"],
                &["fiction"],
                None,
                Some(("image/jpeg", b"NEW")),
            )],
        )
        .await
        .unwrap();

        // No orphan rows in any link table for books that no longer exist.
        for table in [
            "books_authors_link",
            "books_tags_link",
            "books_series_link",
            "books_publishers_link",
            "books_languages_link",
        ] {
            let orphan: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM {table} WHERE book NOT IN (SELECT id FROM books)"
            ))
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(orphan, 0, "{table} should have no orphans");
        }
        let orphan_files: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM book_files WHERE book_id NOT IN (SELECT id FROM books)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(orphan_files, 0);

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1);
        let cover = get_cover(&pool, books[0].id).await.unwrap();
        assert_eq!(cover, Some(("image/jpeg".into(), b"NEW".to_vec())));
    }
    #[tokio::test]
    async fn author_dedupes_across_books_case_insensitive() {
        let _covers = CoversTempDir::new("dedupe");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None),
                indexed("b.epub", Some("B"), &["tolkien"], &[], None, None),
            ],
        )
        .await
        .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "NOCASE unique should collapse Tolkien/tolkien");
    }
    #[tokio::test]
    async fn series_index_sorts_numerically() {
        // Regression guard against reintroducing Calibre's TEXT series_index:
        // 10 must sort after 2, not before.
        let _covers = CoversTempDir::new("series");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("b.epub", Some("B"), &["A"], &[], Some(("S", "10")), None),
                indexed("a.epub", Some("A"), &["A"], &[], Some(("S", "2")), None),
            ],
        )
        .await
        .unwrap();
        let indices: Vec<f64> =
            sqlx::query_scalar("SELECT series_index FROM books ORDER BY series_index")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(indices, vec![2.0, 10.0]);
    }
    #[tokio::test]
    async fn rename_author_updates_fts_via_trigger() {
        let _covers = CoversTempDir::new("fts_rename");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Book"),
                &["OldName"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        assert_eq!(
            search_books(&pool, "/lib", "OldName").await.unwrap().len(),
            1
        );

        sqlx::query("UPDATE authors SET name = 'NewName' WHERE name = 'OldName'")
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            search_books(&pool, "/lib", "NewName").await.unwrap().len(),
            1
        );
        assert_eq!(
            search_books(&pool, "/lib", "OldName").await.unwrap().len(),
            0
        );
    }
    #[tokio::test]
    async fn reindex_keeps_fts_row_count_in_sync() {
        let _covers = CoversTempDir::new("fts_reindex");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["X"], &[], None, None),
                indexed("b.epub", Some("B"), &["Y"], &[], None, None),
            ],
        )
        .await
        .unwrap();
        // Reindex with one fewer book.
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
        )
        .await
        .unwrap();

        let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
            .fetch_one(&pool)
            .await
            .unwrap();
        let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(book_count, 1);
        assert_eq!(fts_count, 1, "FTS row count must match book count");
    }
    #[tokio::test]
    async fn overrides_survive_reindex() {
        let _covers = CoversTempDir::new("reindex_survive");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // First index.
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "survive.epub",
                Some("Original"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();

        // Save overrides.
        let ov = MetadataOverrides {
            title: Some("Overridden".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // Reindex — replace_books deletes and re-inserts.
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "survive.epub",
                Some("Original"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        // The new book row has a new id but the same UUID.
        let books = list_books(&pool, "/lib").await.unwrap();
        let id = books[0].id;
        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(
            merged.title.as_deref(),
            Some("Overridden"),
            "overrides should survive the DELETE/INSERT reindex"
        );
        assert!(merged.has_override);
    }
    #[tokio::test]
    async fn reindex_path_skips_blocked_contributor_and_keeps_real_author() {
        // End-to-end: simulate the reindex path by going through
        // `replace_books` (same insert_metadata_links pipeline). The
        // blocklist guard must keep the junk row from being re-created
        // while the legitimate author is still linked to the book.
        let _covers = CoversTempDir::new("ignored_authors_reindex");
        let pool = init_db("sqlite::memory:").await.unwrap();

        sqlx::query("INSERT INTO ignored_authors(name) VALUES (?)")
            .bind("calibre (8.0.0) [https://calibre-ebook.com]")
            .execute(&pool)
            .await
            .unwrap();

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "the-real-book.epub",
                Some("The Real Book"),
                &["Real Author", "calibre (8.0.0) [https://calibre-ebook.com]"],
                &[],
                None,
                None,
            )],
        )
        .await
        .expect("reindex path should succeed even with a blocked contributor");

        let junk_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE name = ?")
            .bind("calibre (8.0.0) [https://calibre-ebook.com]")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(junk_count, 0, "junk author must not be re-created");

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1);
        let book = &books[0];
        let names: Vec<&str> = book.creators.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Real Author"],
            "only the un-blocked creator should remain linked"
        );
    }

    #[tokio::test]
    async fn reindex_blocklist_matches_case_insensitively() {
        // The batched blocklist join leans on `ignored_authors.name`'s NOCASE
        // collation: a contributor whose casing differs from the stored entry
        // must still be skipped. Guards `insert_author_links` against a
        // regression (e.g. swapping the join operands) that would silently
        // break case-insensitive matching.
        let _covers = CoversTempDir::new("ignored_authors_nocase");
        let pool = init_db("sqlite::memory:").await.unwrap();

        sqlx::query("INSERT INTO ignored_authors(name) VALUES (?)")
            .bind("Calibre Bot")
            .execute(&pool)
            .await
            .unwrap();

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "the-real-book.epub",
                Some("The Real Book"),
                &["Real Author", "calibre bot"],
                &[],
                None,
                None,
            )],
        )
        .await
        .expect("reindex should succeed with a mixed-case blocked contributor");

        let junk_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE name = ? COLLATE NOCASE")
                .bind("calibre bot")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            junk_count, 0,
            "a case-variant blocked author must not be created"
        );

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1);
        let names: Vec<&str> = books[0].creators.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(
            names,
            vec!["Real Author"],
            "the mixed-case blocked contributor must be skipped"
        );
    }
}
