//! Normalized DB layer (server-only).
//!
//! The schema (see `../migrations/0002_normalized_schema.sql`) splits a
//! logical `books` row from one-or-more `book_files` rows per format, and
//! normalizes authors, series, tags, publishers, languages, and identifiers
//! into their own tables joined via m2m link tables. The filesystem remains
//! the source of truth — every row here is rebuildable by reindexing.
//!
//! The public API here preserves the shape older callers expect
//! (`replace_books`, `list_books`, `library_from_db`, `get_cover`,
//! `last_indexed_at`) so the indexer, rpc, and backend layers only need to
//! change at the edges. Internally, each of those functions drives the
//! normalized layout.

use sqlx::{Row, SqlitePool, Transaction};

use omnibus_shared::{
    AuthorDetail, AuthorSummary, EbookMetadata, PaletteAuthorHit, PaletteBookHit, PaletteResults,
    PaletteSeriesHit, PaletteTagHit, SeriesDetail, SeriesSummary, TagWeight,
};

use crate::books::{backfill_creator_ids, parse_json_array, row_to_ebook, BOOK_COLUMNS};
use crate::covers::{delete_cover_files_for, write_cover_file};
use crate::helpers::{
    build_fts_match, join_names, parse_series_index, sanitize_accent_color, split_filename,
    stable_uuid,
};
pub use crate::helpers::sanitize_fts_query;
use crate::metadata_overrides::{apply_overrides, load_overrides_bulk, rebuild_fts_for_book};
use crate::settings::upsert_library;
use crate::taxonomy::{
    resolve_or_insert_author, resolve_or_insert_language, resolve_or_insert_publisher,
    resolve_or_insert_series, resolve_or_insert_tag,
};

/// Hard cap on the nested `books` vec returned by the discovery-detail
/// reads ([`get_author`] / [`get_series`]). Issue #150: these functions
/// previously serialized *every* book attributed to an author or series
/// in one payload — a prolific reference author or a giant Calibre series
/// could nest thousands of `EbookMetadata` structs (each with its own
/// `Vec<Contributor>` / `Vec` subjects / `Vec<Identifier>`) into a single
/// response. 1 000 is far above any realistic single-author/series shelf a
/// client renders in one grid, yet keeps the JSON envelope bounded.
///
/// Truncation is surfaced *without* a struct change: `AuthorDetail` /
/// `SeriesDetail` already carry a `book_count` field. Both reads now set
/// `book_count` from a dedicated uncapped `COUNT(*)`, so a caller detects
/// truncation as `book_count > books.len()`. The `X-Total-Count` header
/// floated in #150 doesn't fit here — these flow through Dioxus server
/// functions (`frontend/src/rpc.rs`), not raw axum handlers, so there's no
/// ergonomic place to set a response header. Cursor pagination is the
/// intended F4.x follow-up; see `docs/roadmap/`.
pub const MAX_DISCOVERY_BOOKS: i64 = 1_000;

// -----------------------------------------------------------------------------
// Indexer write path.
// -----------------------------------------------------------------------------

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


// -----------------------------------------------------------------------------
// Discovery pages (F1.8)
// -----------------------------------------------------------------------------


/// Fetch an author by ID with their books across every library. Returns
/// `None` if the author ID doesn't exist.
///
/// # Bounded reads (issue #150)
///
/// The nested `books` vec is hard-capped at [`MAX_DISCOVERY_BOOKS`]; a
/// prolific reference author no longer serializes thousands of nested
/// `EbookMetadata` structs in one payload. `book_count` is computed from a
/// separate uncapped `COUNT(*)`, so it always reports the true shelf size
/// and callers detect truncation as `book_count > books.len()`. Cursor
/// pagination is the intended F4.x follow-up.
///
/// # Multi-tenancy
///
/// This function returns all matching rows without filtering by the
/// requesting user's library access. The app is single-tenant and
/// single-library today (see Phase 4 in `docs/roadmap/0-0-summary.md`),
/// so every authenticated caller is implicitly authorised to see every
/// author. When per-user ACLs land in the F4.x phase this function must
/// accept a `user_id` parameter and filter both the `authors` row and
/// the joined `books` via the access-control join. The `TODO(F4.x)`
/// marker in the body stays in place until that work lands.
pub async fn get_author(
    pool: &SqlitePool,
    author_id: i64,
) -> Result<Option<AuthorDetail>, sqlx::Error> {
    // TODO(F4.x): scope by `user_id` once per-user ACLs land. See the
    // function-level rustdoc above for the single-tenant rationale.
    let author_row = sqlx::query("SELECT id, name, sort FROM authors WHERE id = ?")
        .bind(author_id)
        .fetch_optional(pool)
        .await?;
    let Some(a) = author_row else {
        return Ok(None);
    };
    let author_name: String = a.get("name");

    // F5.1: membership and ordering follow the merged (override-aware)
    // view, not the raw `books_authors_link` table. `apply_overrides`
    // replaces creators wholesale when the override JSON has the
    // `creators` key, so the effective creator set for a book is:
    //   - `overrides.creators` if the JSON has that key (including the
    //     empty array, which clears all authors), else
    //   - the canonical names from `books_authors_link`.
    // Override creators carry only `name`, so we match by name against
    // the target author. `series_index` overrides drive ordering the same
    // way as in `get_series`.
    let sql = format!(
        r#"SELECT {BOOK_COLUMNS}
           FROM books b
           LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           WHERE
             CASE
               WHEN mo.book_uuid IS NOT NULL
                    AND json_type(mo.overrides, '$.creators') IS NOT NULL
                 THEN EXISTS (
                   SELECT 1 FROM json_each(mo.overrides, '$.creators') je
                    WHERE json_extract(je.value, '$.name') = ? COLLATE NOCASE
                 )
               ELSE EXISTS (
                 SELECT 1 FROM books_authors_link bal
                  WHERE bal.book = b.id AND bal.author = ?
               )
             END
           ORDER BY
             CASE
               WHEN mo.book_uuid IS NOT NULL
                    AND json_type(mo.overrides, '$.series_index') IS NOT NULL
                 -- NULLIF: an override that explicitly clears the index
                 -- (`Some("")` from the edit form) would otherwise CAST to
                 -- 0.0 and sort to the front. Treat empty-string as "no
                 -- index" so ORDER BY's NULLS LAST trails it — matching
                 -- get_series.
                 THEN CAST(NULLIF(json_extract(mo.overrides, '$.series_index'), '') AS REAL)
               ELSE b.series_index
             END NULLS LAST,
             b.sort, b.id
           LIMIT ?"#
    );
    let rows = sqlx::query(&sql)
        .bind(&author_name)
        .bind(author_id)
        .bind(MAX_DISCOVERY_BOOKS)
        .fetch_all(pool)
        .await?;

    let mut books = Vec::with_capacity(rows.len());
    for r in &rows {
        books.push(row_to_ebook(r)?);
    }

    // Issue #150: `books` above is capped at `MAX_DISCOVERY_BOOKS`, so its
    // length can no longer stand in for the author's true shelf size.
    // Count the (uncapped) effective membership separately using the same
    // override-aware predicate as the SELECT so `book_count` stays
    // truthful and callers can detect truncation as
    // `book_count > books.len()`.
    let book_count: i64 = sqlx::query_scalar(
        r#"SELECT COUNT(*)
           FROM books b
           LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           WHERE
             CASE
               WHEN mo.book_uuid IS NOT NULL
                    AND json_type(mo.overrides, '$.creators') IS NOT NULL
                 THEN EXISTS (
                   SELECT 1 FROM json_each(mo.overrides, '$.creators') je
                    WHERE json_extract(je.value, '$.name') = ? COLLATE NOCASE
                 )
               ELSE EXISTS (
                 SELECT 1 FROM books_authors_link bal
                  WHERE bal.book = b.id AND bal.author = ?
               )
             END"#,
    )
    .bind(&author_name)
    .bind(author_id)
    .fetch_one(pool)
    .await?;

    // Bulk-apply overrides so card titles / descriptions / covers reflect
    // user edits, matching what `list_books` does for the landing grid.
    let uuids: Vec<String> = books
        .iter()
        .filter_map(|b| b.unique_identifier.clone())
        .collect();
    let overrides_map = load_overrides_bulk(pool, &uuids).await?;
    for book in &mut books {
        if let Some(uuid) = book.unique_identifier.as_deref() {
            if let Some((ov, has_cover_ov)) = overrides_map.get(uuid) {
                apply_overrides(book, ov, *has_cover_ov);
            }
        }
    }
    backfill_creator_ids(pool, &mut books).await?;

    // F1.11: surface whether a usable profile photo is cached so the
    // frontend can render <img> vs the typographic letter avatar in one
    // round trip. `'letter'` rows are the negative-cache marker and do
    // not count as a usable photo.
    let has_photo: bool = sqlx::query_scalar(
        "SELECT EXISTS(
             SELECT 1 FROM author_photos
              WHERE author_id = ?
                AND source IN ('manual', 'openlibrary')
                AND bytes IS NOT NULL
         )",
    )
    .bind(author_id)
    .fetch_one(pool)
    .await?;

    Ok(Some(AuthorDetail {
        id: a.get("id"),
        name: a.get("name"),
        sort: a.get("sort"),
        book_count: book_count as usize,
        books,
        has_photo,
    }))
}

// -----------------------------------------------------------------------------
// F1.11 Author profile photos.
//
// `author_photos` holds at most one row per author (PK = author_id). The
// `source` column distinguishes:
//   - `'manual'`     — admin upload via `PUT /api/authors/:id/photo`.
//   - `'openlibrary'`— resolved by the background worker.
//   - `'letter'`     — negative-cache marker: no usable image. `bytes` and
//                       `mime` are NULL; `get_author_photo` returns `None`
//                       so the GET handler 404s and the letter avatar
//                       stays in place.
// Admin DELETE clears the row to force re-resolution on next view.
// -----------------------------------------------------------------------------

/// Source-of-truth marker for a cached author photo row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorPhotoSource {
    Manual,
    OpenLibrary,
    Letter,
}

impl AuthorPhotoSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthorPhotoSource::Manual => "manual",
            AuthorPhotoSource::OpenLibrary => "openlibrary",
            AuthorPhotoSource::Letter => "letter",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "manual" => Some(Self::Manual),
            "openlibrary" => Some(Self::OpenLibrary),
            "letter" => Some(Self::Letter),
            _ => None,
        }
    }
}

/// Fetch a cached profile photo for the given author. Returns `None` when no
/// row exists or the row is a `'letter'` negative-cache marker — both cases
/// should produce a 404 from the GET handler so the frontend keeps rendering
/// the letter avatar.
pub async fn get_author_photo(
    pool: &SqlitePool,
    author_id: i64,
) -> Result<Option<(String, Vec<u8>)>, sqlx::Error> {
    // Filter `letter` rows in SQL as well as via the schema CHECK constraint
    // so a malformed row (e.g. left over from a future migration drift)
    // can never accidentally serve `letter` bytes as a real image. Belt +
    // suspenders alongside the table-level invariant.
    let row: Option<(String, Option<String>, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT source, mime, bytes
           FROM author_photos
          WHERE author_id = ? AND source <> 'letter'",
    )
    .bind(author_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((_, Some(mime), Some(bytes))) if !bytes.is_empty() => Ok(Some((mime, bytes))),
        _ => Ok(None),
    }
}

/// Look up just the cascade-state metadata (source + fetched_at) for an
/// author. Used by the resolver to decide whether to skip resolution — a
/// `'letter'` row prevents re-querying Open Library until an admin clears it
/// via `delete_author_photo`.
pub async fn author_photo_status(
    pool: &SqlitePool,
    author_id: i64,
) -> Result<Option<(AuthorPhotoSource, String)>, sqlx::Error> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT source, fetched_at FROM author_photos WHERE author_id = ?")
            .bind(author_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(s, t)| AuthorPhotoSource::parse(&s).map(|src| (src, t))))
}

/// Upsert a photo row. Replaces any existing row (PRIMARY KEY conflict on
/// `author_id`). `bytes` / `mime` are `None` for `'letter'` negative-cache
/// markers; `url` is `None` for manual uploads.
pub async fn upsert_author_photo(
    pool: &SqlitePool,
    author_id: i64,
    source: AuthorPhotoSource,
    url: Option<&str>,
    mime: Option<&str>,
    bytes: Option<&[u8]>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO author_photos (author_id, source, url, mime, bytes, fetched_at)
              VALUES (?, ?, ?, ?, ?, datetime('now'))
         ON CONFLICT(author_id) DO UPDATE SET
              source     = excluded.source,
              url        = excluded.url,
              mime       = excluded.mime,
              bytes      = excluded.bytes,
              fetched_at = excluded.fetched_at",
    )
    .bind(author_id)
    .bind(source.as_str())
    .bind(url)
    .bind(mime)
    .bind(bytes)
    .execute(pool)
    .await?;
    Ok(())
}

/// Drop the cache row for an author so the next page view re-queues
/// resolution. Used by admin DELETE.
pub async fn delete_author_photo(pool: &SqlitePool, author_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM author_photos WHERE author_id = ?")
        .bind(author_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete an author by id and prevent reindex from re-creating the row.
///
/// F5.9-lite durable junk-author removal (issue #159). One transaction:
/// look up the name (for the blocklist insert) and affected book ids
/// (for the post-commit FTS refresh), drop the link rows, drop the
/// `authors` row, then `INSERT OR IGNORE` the name into
/// `ignored_authors` so the next `indexer::reindex` does not silently
/// re-create the row via `resolve_or_insert_author`.
///
/// Returns the number of books that were un-linked (used by the admin
/// confirmation modal to show "this affects N books"). Returns `Ok(0)`
/// without touching the blocklist if `author_id` does not exist — a
/// stale tab firing a second Delete must not leak a ghost row into
/// `ignored_authors`.
///
/// FTS refresh runs best-effort *after* commit, mirroring
/// [`upsert_metadata_overrides`]: a stale FTS row is fixed on the next
/// reindex, but a refresh failure must not undo the admin's intent.
pub async fn delete_author(pool: &SqlitePool, author_id: i64) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;

    let Some(name): Option<String> = sqlx::query_scalar("SELECT name FROM authors WHERE id = ?")
        .bind(author_id)
        .fetch_optional(&mut *tx)
        .await?
    else {
        return Ok(0);
    };

    // Snapshot the affected book UUIDs *inside* the transaction by
    // joining link → books. Doing the lookup as a uuid join (one bind
    // parameter) instead of post-commit `WHERE id IN (?, ?, …)` keeps
    // the post-commit FTS refresh from hitting SQLite's 999 bind-
    // parameter cap on authors linked to >999 books — without that
    // cap a giant-author delete would silently skip its FTS rebuild
    // and leave the index stale.
    let affected_uuids: Vec<String> = sqlx::query_scalar(
        "SELECT b.uuid FROM books b
         JOIN books_authors_link l ON l.book = b.id
         WHERE l.author = ?",
    )
    .bind(author_id)
    .fetch_all(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM books_authors_link WHERE author = ?")
        .bind(author_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM authors WHERE id = ?")
        .bind(author_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query("INSERT OR IGNORE INTO ignored_authors(name) VALUES (?)")
        .bind(&name)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    for uuid in &affected_uuids {
        if let Err(e) = rebuild_fts_for_book(pool, uuid).await {
            tracing::warn!(uuid = uuid.as_str(), error = %e, "books_fts rebuild after delete_author failed");
        }
    }

    Ok(affected_uuids.len() as u64)
}

/// Fetch a series by ID with its books, ordered by series index. Returns
/// `None` if the series ID doesn't exist.
///
/// # Bounded reads (issue #150)
///
/// The nested `books` vec is hard-capped at [`MAX_DISCOVERY_BOOKS`]; a
/// giant Calibre series no longer serializes its entire shelf in one
/// payload. `book_count` is computed from a separate uncapped `COUNT(*)`,
/// so it reports the true series size and truncation is detectable as
/// `book_count > books.len()`. Cursor pagination is the intended F4.x
/// follow-up.
///
/// # Multi-tenancy
///
/// This function returns all matching rows without filtering by the
/// requesting user's library access. The app is single-tenant and
/// single-library today (see Phase 4 in `docs/roadmap/0-0-summary.md`),
/// so every authenticated caller is implicitly authorised to see every
/// series. When per-user ACLs land in the F4.x phase this function must
/// accept a `user_id` parameter and filter both the `series` row and the
/// joined `books` via the access-control join — same caveat as
/// [`get_author`]. The `TODO(F4.x)` marker in the body stays in place
/// until that work lands.
pub async fn get_series(
    pool: &SqlitePool,
    series_id: i64,
) -> Result<Option<SeriesDetail>, sqlx::Error> {
    // TODO(F4.x): scope by `user_id` once per-user ACLs land. See the
    // function-level rustdoc above for the single-tenant rationale.
    let series_row = sqlx::query("SELECT id, name, sort FROM series WHERE id = ?")
        .bind(series_id)
        .fetch_optional(pool)
        .await?;
    let Some(s) = series_row else {
        return Ok(None);
    };
    let series_name: String = s.get("name");

    // F5.1: membership and ordering follow the merged (override-aware)
    // view, not the raw `books_series_link` table. `upsert_metadata_overrides`
    // never writes to the relational link tables, so a book added to a
    // series purely through the edit form would otherwise be invisible
    // here even though `apply_overrides` shows it in that series everywhere
    // else (book detail, landing grid). The effective series for a book is:
    //   - `overrides.series` if the JSON has that key (including the empty
    //     string, which means "clear the series"), else
    //   - the canonical name from `books_series_link`.
    // Same fallback for `series_index` so the override-set position drives
    // ordering when present.
    let sql = format!(
        r#"WITH effective AS (
             SELECT b.id AS book_id,
                    CASE
                      WHEN mo.book_uuid IS NOT NULL
                           AND json_type(mo.overrides, '$.series') IS NOT NULL
                        THEN json_extract(mo.overrides, '$.series')
                      ELSE (SELECT s2.name FROM books_series_link bsl
                              JOIN series s2 ON s2.id = bsl.series
                             WHERE bsl.book = b.id LIMIT 1)
                    END AS series_name,
                    CASE
                      WHEN mo.book_uuid IS NOT NULL
                           AND json_type(mo.overrides, '$.series_index') IS NOT NULL
                        -- NULLIF: an override that explicitly clears the
                        -- index (`Some("")` from the edit form) would
                        -- otherwise CAST to 0.0 and sort to the front of
                        -- the series. Treat empty-string as "no index"
                        -- and let ORDER BY's NULLS LAST trail it.
                        THEN CAST(NULLIF(json_extract(mo.overrides, '$.series_index'), '') AS REAL)
                      ELSE b.series_index
                    END AS series_index
               FROM books b
               LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           )
           SELECT {BOOK_COLUMNS}
           FROM books b
           JOIN effective e ON e.book_id = b.id
           WHERE e.series_name = ? COLLATE NOCASE
           ORDER BY e.series_index NULLS LAST, b.sort, b.id
           LIMIT ?"#
    );
    let rows = sqlx::query(&sql)
        .bind(&series_name)
        .bind(MAX_DISCOVERY_BOOKS)
        .fetch_all(pool)
        .await?;

    let mut books = Vec::with_capacity(rows.len());
    for r in &rows {
        books.push(row_to_ebook(r)?);
    }

    // Issue #150: `books` is capped at `MAX_DISCOVERY_BOOKS`. Count the
    // uncapped effective membership separately — reusing the same
    // override-aware `effective` CTE — so `book_count` reflects the true
    // series size and truncation is detectable as
    // `book_count > books.len()`.
    let book_count: i64 = sqlx::query_scalar(
        r#"WITH effective AS (
             SELECT b.id AS book_id,
                    CASE
                      WHEN mo.book_uuid IS NOT NULL
                           AND json_type(mo.overrides, '$.series') IS NOT NULL
                        THEN json_extract(mo.overrides, '$.series')
                      ELSE (SELECT s2.name FROM books_series_link bsl
                              JOIN series s2 ON s2.id = bsl.series
                             WHERE bsl.book = b.id LIMIT 1)
                    END AS series_name
               FROM books b
               LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
           )
           SELECT COUNT(*)
           FROM effective e
           WHERE e.series_name = ? COLLATE NOCASE"#,
    )
    .bind(&series_name)
    .fetch_one(pool)
    .await?;

    // Merge overrides into each book so the series-card title/description
    // /cover reflect user edits, matching what `list_books` does for the
    // landing grid.
    let uuids: Vec<String> = books
        .iter()
        .filter_map(|b| b.unique_identifier.clone())
        .collect();
    let overrides_map = load_overrides_bulk(pool, &uuids).await?;
    for book in &mut books {
        if let Some(uuid) = book.unique_identifier.as_deref() {
            if let Some((ov, has_cover_ov)) = overrides_map.get(uuid) {
                apply_overrides(book, ov, *has_cover_ov);
            }
        }
        // Pin `series_id` / `series` to the parent series for every
        // returned row, not just the override-only ones. A book that was
        // canonically in series A but overridden into series B would
        // otherwise come back here with `series_id = Some(A)` (from the
        // BOOK_COLUMNS subquery, which only reads `books_series_link`)
        // — so the card on B's page would link back to /series/A. We're
        // already on B's page by construction (the WHERE filter matched
        // the effective series name), so unconditionally pinning to the
        // requested series is correct.
        book.series_id = Some(series_id);
        book.series = Some(series_name.clone());
    }
    backfill_creator_ids(pool, &mut books).await?;

    Ok(Some(SeriesDetail {
        id: s.get("id"),
        name: s.get("name"),
        sort: s.get("sort"),
        book_count: book_count as usize,
        books,
    }))
}

/// Maximum number of tags returned from [`get_tag_cloud`]. Caps the
/// payload so a Calibre dump with 10k+ unique subjects can't blow up
/// the client or stall the SQLite pool on serialization.
const TAG_CLOUD_LIMIT: i64 = 500;

/// Return up to [`TAG_CLOUD_LIMIT`] tags with their book counts, ordered
/// by count descending then name ascending. Used by the tag cloud page.
///
/// # Bounded reads (issue #150)
///
/// Unlike [`get_author`] / [`get_series`], this read never nests book
/// lists — it returns only `TagWeight { name, count }` rows — and the tag
/// list itself is already capped at [`TAG_CLOUD_LIMIT`]. There is nothing
/// unbounded to cap here; the #150 work is the discovery-detail reads.
///
/// # Multi-tenancy
///
/// This function returns the global tag distribution without filtering
/// by the requesting user's library access. The app is single-tenant and
/// single-library today (see Phase 4 in `docs/roadmap/0-0-summary.md`),
/// so every authenticated caller sees the same tag cloud. When per-user
/// ACLs land in the F4.x phase this function must accept a `user_id`
/// parameter and count only books visible to that user via the
/// access-control join. The `TODO(F4.x)` marker in the body stays in
/// place until that work lands.
pub async fn get_tag_cloud(pool: &SqlitePool) -> Result<Vec<TagWeight>, sqlx::Error> {
    // TODO(F4.x): scope by `user_id` once per-user ACLs land. See the
    // function-level rustdoc above for the single-tenant rationale.
    //
    // F5.1: counts use the effective (override-aware) subject set, not
    // the raw `books_tags_link` rows — `overrides.subjects` replaces a
    // book's canonical tag list wholesale when Some. Visibility still
    // requires at least one canonical link so tags that exist only as a
    // string inside override JSON don't surface (no `tags` row to point
    // at). The single-library, single-tenant scope means the cloud is
    // global; if/when per-library scoping lands this picks up a path
    // filter alongside the existing `WHERE EXISTS`.
    let rows = sqlx::query(
        r#"SELECT t.name,
             (SELECT COUNT(*)
                FROM books b
                LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
               WHERE CASE
                       WHEN mo.book_uuid IS NOT NULL
                            AND json_type(mo.overrides, '$.subjects') IS NOT NULL
                         THEN EXISTS (
                           SELECT 1 FROM json_each(mo.overrides, '$.subjects') je
                            WHERE je.value = t.name
                         )
                       ELSE EXISTS (
                         SELECT 1 FROM books_tags_link btl
                          WHERE btl.book = b.id AND btl.tag = t.id
                       )
                     END
             ) AS cnt
           FROM tags t
           WHERE EXISTS (
             SELECT 1 FROM books_tags_link btl WHERE btl.tag = t.id
           )
           ORDER BY cnt DESC, t.name ASC
           LIMIT ?"#,
    )
    .bind(TAG_CLOUD_LIMIT)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| TagWeight {
            name: r.get("name"),
            count: r.get::<i64, _>("cnt") as usize,
        })
        .collect())
}

// -----------------------------------------------------------------------------
// Index pages (F1.12) — /authors and /series
// -----------------------------------------------------------------------------

/// Hard cap on rows returned by [`list_authors`] / [`list_series`]. The
/// F1.12 roadmap notes a 5k+ author library is the upper bound we want to
/// keep responsive on a single page; 10k leaves headroom while keeping
/// the JSON envelope under ~1 MB even with the optional accent string.
const INDEX_LIMIT: i64 = 10_000;

/// Return every author with their book count and an optional cover-derived
/// accent, scoped to `library_path`. Empty list when `library_path` does
/// not match a configured library.
///
/// Ordered by name ascending. The UI does its own client-side sort/filter,
/// so this returns the full list up to [`INDEX_LIMIT`] — see
/// [docs/roadmap/1-12-browse-authors-series.md] for the per-library size
/// expectations.
///
/// Accent: the `accent_color` of the first book by this author with a
/// non-null value, by book sort/id order. `NULL` is returned when no book
/// has one, and the UI falls back to the theme accent.
///
/// # Multi-tenancy
///
/// Single-tenant today — every authenticated caller sees the same list.
/// When per-user ACLs land in F4.x this function must accept a `user_id`
/// and join through the access-control table the same way [`get_author`]
/// will.
pub async fn list_authors(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Vec<AuthorSummary>, sqlx::Error> {
    // TODO(F4.x): scope by `user_id` once per-user ACLs land.
    //
    // F5.1: count uses the effective (override-aware) creator set, not
    // the raw `books_authors_link` rows — otherwise an author whose books
    // were all reassigned through the metadata edit form keeps reporting
    // the canonical count even though `/authors/:id` (override-aware
    // since #153) shows the corrected list. Visibility still requires
    // at least one canonical link in this library so we don't list
    // authors that exist only inside override JSON — they'd have no
    // navigable `authors.id`. The accent subquery is unchanged: accent
    // comes from `books.accent_color` (cover-derived), which overrides
    // can replace via `override_covers/<uuid>` materialisation but never
    // by JSON edit, so the canonical-link lookup is still correct.
    //
    // Same shape as the palette author query (`search_palette`'s B
    // block) so the two index surfaces report consistent counts.
    let rows = sqlx::query(
        r#"
        SELECT a.id, a.name, a.sort,
               (SELECT COUNT(*)
                  FROM books b
                  JOIN libraries l2 ON l2.id = b.library_id
                  LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
                 WHERE l2.path = ?1
                   AND CASE
                         WHEN mo.book_uuid IS NOT NULL
                              AND json_type(mo.overrides, '$.creators') IS NOT NULL
                           THEN EXISTS (
                             SELECT 1 FROM json_each(mo.overrides, '$.creators') je
                              WHERE json_extract(je.value, '$.name') = a.name COLLATE NOCASE
                           )
                         ELSE EXISTS (
                           SELECT 1 FROM books_authors_link bal
                            WHERE bal.book = b.id AND bal.author = a.id
                         )
                       END
               ) AS book_count,
               (SELECT b2.accent_color
                  FROM books_authors_link bal2
                  JOIN books b2 ON b2.id = bal2.book
                  JOIN libraries l2 ON l2.id = b2.library_id
                 WHERE bal2.author = a.id
                   AND l2.path = ?1
                   AND b2.accent_color IS NOT NULL
                 ORDER BY b2.sort, b2.id
                 LIMIT 1) AS accent,
               EXISTS(
                 SELECT 1 FROM author_photos ap
                  WHERE ap.author_id = a.id
                    AND ap.source IN ('manual', 'openlibrary')
                    AND ap.bytes IS NOT NULL
               ) AS has_photo
        FROM authors a
        WHERE EXISTS (
            SELECT 1 FROM books_authors_link bal
              JOIN books b ON b.id = bal.book
              JOIN libraries l ON l.id = b.library_id
             WHERE bal.author = a.id AND l.path = ?1
          )
        ORDER BY COALESCE(a.sort, a.name) COLLATE NOCASE ASC
        LIMIT ?2
        "#,
    )
    .bind(library_path)
    .bind(INDEX_LIMIT)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| AuthorSummary {
            id: r.get("id"),
            name: r.get("name"),
            sort: r.get("sort"),
            book_count: r.get::<i64, _>("book_count") as usize,
            accent: r.get("accent"),
            has_photo: r.get::<i64, _>("has_photo") != 0,
        })
        .collect())
}

/// Return every series with book count, primary author, and an optional
/// accent, scoped to `library_path`.
///
/// `primary_author` is the first creator of the lowest-`series_index`
/// book in the series (with `book.sort, book.id` as deterministic
/// tie-breakers). It can be `None` when every book in the series has only
/// override-supplied creators that haven't been linked to an `authors`
/// row yet — surface a plain text by-line in that case.
///
/// Ordered by name ascending. Capped at [`INDEX_LIMIT`].
///
/// # Multi-tenancy
///
/// Same single-tenant caveat as [`list_authors`].
pub async fn list_series(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Vec<SeriesSummary>, sqlx::Error> {
    // TODO(F4.x): scope by `user_id` once per-user ACLs land.
    //
    // F5.1: same overlay shape as `list_authors` and the palette series
    // query. `overrides.series` (string) drives membership for `book_count`;
    // `overrides.creators[0].name` drives `primary_author` when present so
    // the by-line on the card matches what `/series/:id` (#153 + the
    // empty-index follow-up) shows. Visibility still requires at least one
    // canonical `books_series_link` row in this library — series that
    // exist only as a string inside override JSON have no `series.id` to
    // route to. Accent comes from cover-derived `books.accent_color`,
    // which JSON overrides can't change, so the canonical-link lookup is
    // still correct.
    let rows = sqlx::query(
        r#"
        SELECT s.id, s.name, s.sort,
               (SELECT COUNT(*)
                  FROM books b
                  JOIN libraries l2 ON l2.id = b.library_id
                  LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
                 WHERE l2.path = ?1
                   AND CASE
                         WHEN mo.book_uuid IS NOT NULL
                              AND json_type(mo.overrides, '$.series') IS NOT NULL
                           THEN json_extract(mo.overrides, '$.series') = s.name COLLATE NOCASE
                         ELSE EXISTS (
                           SELECT 1 FROM books_series_link bsl
                            WHERE bsl.book = b.id AND bsl.series = s.id
                         )
                       END
               ) AS book_count,
               (SELECT
                  CASE
                    WHEN mo2.book_uuid IS NOT NULL
                         AND json_type(mo2.overrides, '$.creators') IS NOT NULL
                      THEN json_extract(mo2.overrides, '$.creators[0].name')
                    ELSE (SELECT a.name FROM books_authors_link bal
                            JOIN authors a ON a.id = bal.author
                           WHERE bal.book = b2.id
                           ORDER BY bal.position LIMIT 1)
                  END
                FROM books_series_link bsl2
                  JOIN books b2 ON b2.id = bsl2.book
                  JOIN libraries l2 ON l2.id = b2.library_id
                  LEFT JOIN metadata_overrides mo2 ON mo2.book_uuid = b2.uuid
                 WHERE bsl2.series = s.id AND l2.path = ?1
                 ORDER BY b2.series_index NULLS LAST, b2.sort, b2.id
                 LIMIT 1) AS primary_author,
               (SELECT b3.accent_color
                  FROM books_series_link bsl3
                  JOIN books b3 ON b3.id = bsl3.book
                  JOIN libraries l3 ON l3.id = b3.library_id
                 WHERE bsl3.series = s.id
                   AND l3.path = ?1
                   AND b3.accent_color IS NOT NULL
                 ORDER BY b3.series_index NULLS LAST, b3.sort, b3.id
                 LIMIT 1) AS accent
        FROM series s
        WHERE EXISTS (
            SELECT 1 FROM books_series_link bsl
              JOIN books b ON b.id = bsl.book
              JOIN libraries l ON l.id = b.library_id
             WHERE bsl.series = s.id AND l.path = ?1
          )
        ORDER BY COALESCE(s.sort, s.name) COLLATE NOCASE ASC
        LIMIT ?2
        "#,
    )
    .bind(library_path)
    .bind(INDEX_LIMIT)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| SeriesSummary {
            id: r.get("id"),
            name: r.get("name"),
            sort: r.get("sort"),
            book_count: r.get::<i64, _>("book_count") as usize,
            primary_author: r.get("primary_author"),
            accent: r.get("accent"),
        })
        .collect())
}

// -----------------------------------------------------------------------------
// Search palette (F1.5)
// -----------------------------------------------------------------------------

/// Maximum query length (in chars) accepted by `search_palette`. Inputs
/// beyond this are truncated to bound FTS5 expression size and LIKE
/// pattern length.
const MAX_QUERY_LEN: usize = 256;

/// Search palette — grouped results for the command-palette overlay (F1.5).
///
/// Returns up to 5 results per category (books, authors, series, tags),
/// with server-side timing in `duration_ms`. Books are matched via FTS5
/// (`build_fts_match`); taxonomy categories use `LIKE '%q%'` against the
/// name column. All results are scoped to `library_path`.
///
/// Returns `PaletteResults::default()` for empty/whitespace queries.
pub async fn search_palette(
    pool: &SqlitePool,
    library_path: &str,
    q: &str,
) -> Result<PaletteResults, sqlx::Error> {
    let trimmed = q.trim();
    if trimmed.is_empty() {
        return Ok(PaletteResults::default());
    }
    // Truncate to cap FTS5 + LIKE expression size. Collect chars to
    // avoid slicing mid-codepoint.
    let trimmed: String = trimmed.chars().take(MAX_QUERY_LEN).collect();
    let trimmed = trimmed.as_str();

    let start = std::time::Instant::now();
    const LIMIT: i32 = 5;

    // A. Books — FTS5 MATCH with BM25 ranking, slim projection. After
    // hydration we overlay metadata_overrides so the title and author line
    // shown in the palette match the merged values the rest of the app
    // displays (FTS already matches on the merged text — see
    // `rebuild_fts_for_book` in the override write path).
    let books = if let Some(match_expr) = build_fts_match(trimmed) {
        let rows = sqlx::query(
            r#"
            SELECT b.id, b.uuid, b.title, b.has_cover, b.accent_color,
                   SUBSTR(b.pubdate, 1, 4) AS year,

                   (SELECT GROUP_CONCAT(a.name, ', ')
                      FROM (SELECT a2.name FROM books_authors_link bal
                              JOIN authors a2 ON a2.id = bal.author
                             WHERE bal.book = b.id
                             ORDER BY bal.position) a)          AS author_display,

                   (SELECT json_group_array(format)
                      FROM (SELECT format FROM book_files
                             WHERE book_id = b.id
                             ORDER BY format))                  AS formats_json

            FROM books_fts
            JOIN books b ON b.id = books_fts.rowid
            JOIN libraries l ON l.id = b.library_id
            WHERE books_fts MATCH ?1 AND l.path = ?2
            ORDER BY bm25(books_fts, 10.0, 4.0, 3.0, 1.0, 1.0, 1.0), b.sort, b.id
            LIMIT ?3
            "#,
        )
        .bind(&match_expr)
        .bind(library_path)
        .bind(LIMIT)
        .fetch_all(pool)
        .await?;

        let mut uuids: Vec<String> = Vec::with_capacity(rows.len());
        let mut hits: Vec<PaletteBookHit> = Vec::with_capacity(rows.len());
        for r in rows.iter() {
            let id: i64 = r.get("id");
            let uuid: String = r.get("uuid");
            let has_cover: i64 = r.get("has_cover");
            uuids.push(uuid.clone());
            hits.push(PaletteBookHit {
                id,
                uuid: uuid.clone(),
                title: r.get::<Option<String>, _>("title").unwrap_or_default(),
                author_display: r
                    .get::<Option<String>, _>("author_display")
                    .unwrap_or_default(),
                year: r.get("year"),
                formats: parse_json_array(r.get("formats_json"))?,
                cover_url: (has_cover != 0).then(|| format!("/api/covers/{uuid}")),
                accent: r.get("accent_color"),
            });
        }

        let overrides_map = load_overrides_bulk(pool, &uuids).await?;
        for (hit, uuid) in hits.iter_mut().zip(uuids.iter()) {
            if let Some((ov, has_cover_ov)) = overrides_map.get(uuid) {
                if let Some(ref t) = ov.title {
                    hit.title = t.clone();
                }
                if let Some(ref creators) = ov.creators {
                    hit.author_display = creators
                        .iter()
                        .map(|c| c.name.as_str())
                        .collect::<Vec<_>>()
                        .join(", ");
                }
                // Mirror `apply_overrides`: surface user-uploaded covers even
                // when the scanned book had `has_cover = 0`.
                if *has_cover_ov {
                    hit.cover_url = Some(format!("/api/covers/{}", hit.uuid));
                }
            }
        }

        hits
    } else {
        Vec::new()
    };

    // Escape the query for LIKE pattern: backslash first (it's the ESCAPE char),
    // then the LIKE wildcards percent and underscore.
    let like_q = trimmed
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let like_pattern = format!("%{like_q}%");

    // B. Authors — substring match, scoped to library, ordered by book count.
    //
    // Library scoping (`l.path = ?1`) is applied before aggregation so
    // book_count stays library-correct (covered by `palette_scoped_to_library`
    // and `palette_taxonomy_counts_scoped_per_library`). The join plan is
    // locked in by `palette_taxonomy_query_plans_use_indexes`.
    //
    // F5.1: the count uses the effective (override-aware) creator set, not
    // the raw `books_authors_link` rows — otherwise an author whose books
    // were all reassigned through the metadata edit form (e.g. "Sanderson,
    // Brandon" → "Brandon Sanderson") keeps reporting the canonical count
    // even though `/author/:id` shows zero. Visibility still requires at
    // least one canonical link row in this library so we don't list
    // authors that exist only as a string inside override JSON (no
    // navigable id), matching the rest of the palette's behavior.
    let authors: Vec<PaletteAuthorHit> = sqlx::query(
        r#"
        SELECT a.id, a.name,
          (SELECT COUNT(*)
             FROM books b
             JOIN libraries l2 ON l2.id = b.library_id
             LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
            WHERE l2.path = ?1
              AND CASE
                    WHEN mo.book_uuid IS NOT NULL
                         AND json_type(mo.overrides, '$.creators') IS NOT NULL
                      THEN EXISTS (
                        SELECT 1 FROM json_each(mo.overrides, '$.creators') je
                         WHERE json_extract(je.value, '$.name') = a.name
                      )
                    ELSE EXISTS (
                      SELECT 1 FROM books_authors_link bal
                       WHERE bal.book = b.id AND bal.author = a.id
                    )
                  END
          ) AS book_count
        FROM authors a
        WHERE a.name LIKE ?2 ESCAPE '\'
          AND EXISTS (
            SELECT 1 FROM books_authors_link bal
              JOIN books b ON b.id = bal.book
              JOIN libraries l ON l.id = b.library_id
             WHERE bal.author = a.id AND l.path = ?1
          )
        ORDER BY book_count DESC, a.name
        LIMIT ?3
        "#,
    )
    .bind(library_path)
    .bind(&like_pattern)
    .bind(LIMIT)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| PaletteAuthorHit {
        id: r.get("id"),
        name: r.get("name"),
        book_count: r.get::<i32, _>("book_count") as u32,
    })
    .collect();

    // C. Series — substring match with primary author from first book.
    //
    // F5.1: both the count and the `author_display` line use the effective
    // (override-aware) view, mirroring `get_series` and the palette author
    // count. `overrides.series` (string) drives membership; if a book's
    // first creator was renamed through the metadata edit form,
    // `overrides.creators[0].name` drives the displayed author. Visibility
    // still requires at least one canonical link in this library so we
    // don't list series that exist only inside override JSON (no
    // navigable id).
    let series: Vec<PaletteSeriesHit> = sqlx::query(
        r#"
        SELECT s.id, s.name,
          (SELECT COUNT(*)
             FROM books b
             JOIN libraries l2 ON l2.id = b.library_id
             LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
            WHERE l2.path = ?1
              AND CASE
                    WHEN mo.book_uuid IS NOT NULL
                         AND json_type(mo.overrides, '$.series') IS NOT NULL
                      THEN json_extract(mo.overrides, '$.series') = s.name
                    ELSE EXISTS (
                      SELECT 1 FROM books_series_link bsl
                       WHERE bsl.book = b.id AND bsl.series = s.id
                    )
                  END
          ) AS book_count,
          (SELECT
             CASE
               WHEN mo2.book_uuid IS NOT NULL
                    AND json_type(mo2.overrides, '$.creators') IS NOT NULL
                 THEN json_extract(mo2.overrides, '$.creators[0].name')
               ELSE (SELECT a.name FROM books_authors_link bal
                       JOIN authors a ON a.id = bal.author
                      WHERE bal.book = b2.id
                      ORDER BY bal.position LIMIT 1)
             END
           FROM books_series_link bsl2
             JOIN books b2 ON b2.id = bsl2.book
             JOIN libraries l2 ON l2.id = b2.library_id
             LEFT JOIN metadata_overrides mo2 ON mo2.book_uuid = b2.uuid
            WHERE bsl2.series = s.id AND l2.path = ?1
            ORDER BY b2.sort, b2.id LIMIT 1) AS author_display
        FROM series s
        WHERE s.name LIKE ?2 ESCAPE '\'
          AND EXISTS (
            SELECT 1 FROM books_series_link bsl
              JOIN books b ON b.id = bsl.book
              JOIN libraries l ON l.id = b.library_id
             WHERE bsl.series = s.id AND l.path = ?1
          )
        ORDER BY book_count DESC, s.name
        LIMIT ?3
        "#,
    )
    .bind(library_path)
    .bind(&like_pattern)
    .bind(LIMIT)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| PaletteSeriesHit {
        id: r.get("id"),
        name: r.get("name"),
        book_count: r.get::<i32, _>("book_count") as u32,
        author_display: r.get("author_display"),
    })
    .collect();

    // D. Tags — substring match, scoped to library.
    //
    // F5.1: the count uses the effective (override-aware) subject set,
    // not the raw `books_tags_link` rows. `MetadataOverrides.subjects`
    // (Option<Vec<String>>) replaces the canonical tag list wholesale
    // when Some — including the empty array, which clears all tags.
    // Visibility still requires at least one canonical link in this
    // library so we don't list tags that exist only inside override
    // JSON (no navigable id).
    let tags: Vec<PaletteTagHit> = sqlx::query(
        r#"
        SELECT t.id, t.name,
          (SELECT COUNT(*)
             FROM books b
             JOIN libraries l2 ON l2.id = b.library_id
             LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
            WHERE l2.path = ?1
              AND CASE
                    WHEN mo.book_uuid IS NOT NULL
                         AND json_type(mo.overrides, '$.subjects') IS NOT NULL
                      THEN EXISTS (
                        SELECT 1 FROM json_each(mo.overrides, '$.subjects') je
                         WHERE je.value = t.name
                      )
                    ELSE EXISTS (
                      SELECT 1 FROM books_tags_link btl
                       WHERE btl.book = b.id AND btl.tag = t.id
                    )
                  END
          ) AS book_count
        FROM tags t
        WHERE t.name LIKE ?2 ESCAPE '\'
          AND EXISTS (
            SELECT 1 FROM books_tags_link btl
              JOIN books b ON b.id = btl.book
              JOIN libraries l ON l.id = b.library_id
             WHERE btl.tag = t.id AND l.path = ?1
          )
        ORDER BY book_count DESC, t.name
        LIMIT ?3
        "#,
    )
    .bind(library_path)
    .bind(&like_pattern)
    .bind(LIMIT)
    .fetch_all(pool)
    .await?
    .iter()
    .map(|r| PaletteTagHit {
        id: r.get("id"),
        name: r.get("name"),
        book_count: r.get::<i32, _>("book_count") as u32,
    })
    .collect();

    let duration_ms = start.elapsed().as_millis() as u64;

    Ok(PaletteResults {
        query: trimmed.to_string(),
        books,
        authors,
        series,
        tags,
        duration_ms,
    })
}


// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::books::{
        count_books, count_search_books, get_book, get_book_by_uuid, library_from_db,
        library_from_db_with_total, list_books, list_indexed_rows, resolve_book_id_by_uuid,
        search_books, IndexedRow, MAX_BOOKS_RETURNED,
    };
    use crate::covers::test_helpers::CoversTempDir;
    use crate::covers::{cover_path_for, get_cover, get_last_modified_epoch};
    use crate::metadata_overrides::{
        delete_metadata_overrides, get_book_uuid, get_metadata_overrides, merge_metadata_overrides,
        upsert_metadata_overrides, write_override_cover,
    };
    use crate::pool::init_db;
    use crate::settings::{
        last_indexed_at, prune_orphan_libraries, seed_settings_from_env, set_settings, Settings,
    };
    use crate::books::sanitize_description;
    use omnibus_shared::{Contributor, Identifier, MetadataOverrides};

    use crate::ebook::IndexedBook;

    fn indexed(
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

    // ------------------------------------------------------------------
    // sync_books — incremental write path. Each test seeds an initial
    // state via `replace_books` (the legacy nuke-and-pave wrapper), then
    // applies a hand-built `SyncPlan` to exercise one or more diff
    // buckets and asserts the post-state.
    // ------------------------------------------------------------------

    /// Build an `IndexedBook` matching `indexed(...)` but with the
    /// supplied (mtime_epoch, size_bytes). Used to drive the New +
    /// Changed branches of sync_books with realistic fs metadata.
    fn indexed_with_stat(
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
    async fn cover_returns_none_when_file_missing() {
        let _covers = CoversTempDir::new("missing");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["A"],
                &[],
                None,
                Some(("image/jpeg", b"BYTES")),
            )],
        )
        .await
        .unwrap();
        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
            .bind(books[0].id)
            .fetch_one(&pool)
            .await
            .unwrap();
        // Remove the file out from under the DB — get_cover should report
        // None, not error.
        let _ = std::fs::remove_file(cover_path_for(&uuid, "jpg"));
        assert!(get_cover(&pool, books[0].id).await.unwrap().is_none());
    }

    /// F5.1 / #107: when a `metadata_overrides` row sets
    /// `has_cover_override = 1` and an `override-<uuid>.<ext>` file exists
    /// on disk, `get_cover` returns the override bytes — not the scanned
    /// cover. Single-query form must preserve this precedence.
    #[tokio::test]
    async fn cover_returns_override_when_flag_set() {
        let _covers = CoversTempDir::new("override_set");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["A"],
                &[],
                None,
                Some(("image/jpeg", b"ORIGINAL")),
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();

        // Mark cover-override + drop the override file on disk.
        write_override_cover(&uuid, "image/png", b"OVERRIDE").unwrap();
        upsert_metadata_overrides(&pool, &uuid, &MetadataOverrides::default(), true, user_id)
            .await
            .unwrap();

        let cover = get_cover(&pool, books[0].id).await.unwrap();
        assert_eq!(cover, Some(("image/png".into(), b"OVERRIDE".to_vec())));
    }

    /// F5.1 / #107: with no `metadata_overrides` row, `get_cover` falls
    /// through to the scanned `<uuid>.<ext>` cover. The LEFT JOIN must
    /// not filter the book out when no override row exists.
    #[tokio::test]
    async fn cover_returns_original_when_no_override_row() {
        let _covers = CoversTempDir::new("override_absent");
        let pool = init_db("sqlite::memory:").await.unwrap();

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["A"],
                &[],
                None,
                Some(("image/jpeg", b"ORIGINAL")),
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let cover = get_cover(&pool, books[0].id).await.unwrap();
        assert_eq!(cover, Some(("image/jpeg".into(), b"ORIGINAL".to_vec())));
    }

    /// F5.1 / #107: a `metadata_overrides` row with
    /// `has_cover_override = 0` (text-only edits, no cover swap) must
    /// resolve to the scanned cover, not the override path.
    #[tokio::test]
    async fn cover_returns_original_when_override_flag_unset() {
        let _covers = CoversTempDir::new("override_flag_off");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["A"],
                &[],
                None,
                Some(("image/jpeg", b"ORIGINAL")),
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();

        // Override row exists with text edits but no cover swap.
        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                title: Some("Edited".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let cover = get_cover(&pool, books[0].id).await.unwrap();
        assert_eq!(cover, Some(("image/jpeg".into(), b"ORIGINAL".to_vec())));
    }

    /// #107: `get_cover` for a non-existent book id returns `Ok(None)`
    /// (not an error). The LEFT JOIN must not change this contract.
    #[tokio::test]
    async fn cover_returns_none_for_missing_book_id() {
        let _covers = CoversTempDir::new("missing_book");
        let pool = init_db("sqlite::memory:").await.unwrap();
        assert!(get_cover(&pool, 999_999).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn library_from_db_returns_empty_for_none_path() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let lib = library_from_db(&pool, None).await.unwrap();
        assert!(lib.path.is_none());
        assert!(lib.books.is_empty());
        assert!(lib.error.is_none());
    }

    // ---------- Server-side cap (issue #81) ----------
    //
    // `list_books` / `search_books` previously had no `LIMIT`, so a single
    // `/api/ebooks` poll on a multi-thousand-book library serialized the
    // whole table. The fix is a hard `LIMIT MAX_BOOKS_RETURNED`, plus a
    // companion count helper so callers can detect truncation.

    /// Seed `count` minimal `books` rows under `/lib` using a recursive CTE.
    /// Bypasses `replace_books` / the indexer entirely — the cap behavior
    /// only depends on rows existing, not on full m2m relations being set
    /// up. Keeps the test runtime down to milliseconds even for 50k+ rows.
    async fn seed_minimal_books(pool: &SqlitePool, count: i64) {
        sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
            .execute(pool)
            .await
            .unwrap();
        let lib_id: i64 = sqlx::query_scalar("SELECT id FROM libraries WHERE path = '/lib'")
            .fetch_one(pool)
            .await
            .unwrap();
        sqlx::query(
            r#"
            WITH RECURSIVE n(i) AS (
                SELECT 1
                UNION ALL
                SELECT i + 1 FROM n WHERE i < ?
            )
            INSERT INTO books (uuid, library_id, path, title, sort)
            SELECT 'uuid-' || i, ?, '/lib/b' || i, 'Title ' || i,
                   'Title ' || printf('%010d', i)
              FROM n
            "#,
        )
        .bind(count)
        .bind(lib_id)
        .execute(pool)
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn list_books_caps_response_at_max_books_returned() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let total = MAX_BOOKS_RETURNED + 25;
        seed_minimal_books(&pool, total).await;

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(
            books.len() as i64,
            MAX_BOOKS_RETURNED,
            "list_books must cap the returned vec at MAX_BOOKS_RETURNED"
        );

        let counted = count_books(&pool, "/lib").await.unwrap();
        assert_eq!(
            counted, total,
            "count_books must report the true row count (uncapped)"
        );
    }

    #[tokio::test]
    async fn count_books_returns_zero_for_unknown_library() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        assert_eq!(count_books(&pool, "/nope").await.unwrap(), 0);
    }

    #[tokio::test]
    async fn library_from_db_with_total_reports_truncation() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let total = MAX_BOOKS_RETURNED + 7;
        seed_minimal_books(&pool, total).await;

        let (lib, returned_total) = library_from_db_with_total(&pool, Some("/lib"))
            .await
            .unwrap();
        assert_eq!(lib.path.as_deref(), Some("/lib"));
        assert_eq!(lib.books.len() as i64, MAX_BOOKS_RETURNED);
        assert_eq!(returned_total, total);
        assert!(
            returned_total > MAX_BOOKS_RETURNED,
            "test must seed strictly more rows than the cap"
        );
    }

    #[tokio::test]
    async fn library_from_db_with_total_reports_zero_for_none_path() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let (lib, total) = library_from_db_with_total(&pool, None).await.unwrap();
        assert!(lib.path.is_none());
        assert!(lib.books.is_empty());
        assert_eq!(total, 0);
    }

    #[tokio::test]
    async fn search_books_finds_by_title_and_ranks_by_bm25() {
        let _covers = CoversTempDir::new("fts_title");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Harry Potter"),
                    &["J.K. Rowling"],
                    &[],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Something Else"),
                    &["Author B"],
                    &["harry"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "harry").await.unwrap();
        // Column filter scopes MATCH to title/authors/series — the tag-only
        // hit on "Something Else" is intentionally excluded.
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Harry Potter"));
    }

    #[tokio::test]
    async fn search_books_finds_by_author_and_scopes_to_library() {
        let _covers = CoversTempDir::new("fts_author");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib-a",
            vec![indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None)],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![indexed("b.epub", Some("B"), &["Tolkien"], &[], None, None)],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib-a", "tolkien").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("A"));
    }

    #[tokio::test]
    async fn search_books_empty_query_returns_empty_vec() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let hits = search_books(&pool, "/lib", "   ").await.unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_books_handles_unbalanced_quote_without_error() {
        let _covers = CoversTempDir::new("fts_unbalanced");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Quoted"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        // Unbalanced `"` in raw input must not surface as a MATCH parse error.
        let hits = search_books(&pool, "/lib", "say \"hi")
            .await
            .expect("sanitizer guards against MATCH parse errors");
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_books_excludes_isbn_column_from_match() {
        // ISBN is indexed in books_fts but the search column filter scopes
        // MATCH to title/authors/series, so ISBN lookups are intentionally
        // not surfaced. When/if we re-enable ISBN search, flip this to
        // assert a hit — no migration required.
        let _covers = CoversTempDir::new("fts_isbn");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let mut meta = indexed("a.epub", Some("ISBN Book"), &["A"], &[], None, None).metadata;
        meta.identifiers.push(Identifier {
            value: "978-0-123456-78-9".into(),
            scheme: Some("isbn".into()),
        });
        replace_books(
            &pool,
            "/lib",
            vec![IndexedBook {
                metadata: meta,
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "978-0-123456-78-9")
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_books_author_facet_filters_to_matching_author() {
        let _covers = CoversTempDir::new("fts_facet_author");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Pride and Prejudice"),
                    &["Austen"],
                    &[],
                    None,
                    None,
                ),
                indexed("b.epub", Some("Hamlet"), &["Shakespeare"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "author:austen").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Pride and Prejudice"));
    }

    #[tokio::test]
    async fn search_books_series_facet_filters_to_matching_series() {
        let _covers = CoversTempDir::new("fts_facet_series");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dune"),
                    &["Herbert"],
                    &[],
                    Some(("Dune Saga", "1")),
                    None,
                ),
                indexed("b.epub", Some("Standalone"), &["Author"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "series:dune").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Dune"));
    }

    #[tokio::test]
    async fn search_books_tag_facet_filters_to_matching_tag() {
        let _covers = CoversTempDir::new("fts_facet_tag");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["X"], &["fiction"], None, None),
                indexed("b.epub", Some("B"), &["Y"], &["history"], None, None),
            ],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "tag:fiction").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("A"));
    }

    #[tokio::test]
    async fn search_books_facet_combines_with_free_text_via_explicit_and() {
        let _covers = CoversTempDir::new("fts_facet_combined");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Pride and Prejudice"),
                    &["Austen"],
                    &[],
                    None,
                    None,
                ),
                indexed("b.epub", Some("Emma"), &["Austen"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        // Both clauses must match — only Pride and Prejudice carries the
        // "pride" token in title/authors/series.
        let hits = search_books(&pool, "/lib", "author:austen pride")
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Pride and Prejudice"));
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
    async fn list_books_populates_formats_from_book_files() {
        // Regression: F1.7 power-user table & inline format chips read
        // `EbookMetadata.formats` off the list endpoint; if list_books
        // returned `vec![]` the chip row would hide itself entirely.
        let _covers = CoversTempDir::new("list_books_formats");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1);
        assert_eq!(books[0].formats, vec!["EPUB".to_string()]);
    }

    #[tokio::test]
    async fn list_books_returns_one_row_per_book_with_multi_format() {
        // Regression for PR #74 review: adding a second physical file
        // (EPUB + M4B) used to duplicate the parent row because the outer
        // query LEFT-JOINed `book_files`. The chip facets / table would
        // then over-count. Both queries now use scalar subqueries so the
        // result is one row per `books.id` and `.formats` carries both.
        let _covers = CoversTempDir::new("list_books_multi");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let id = list_books(&pool, "/lib").await.unwrap()[0].id;
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime)
             VALUES (?, 'M4B', 'alpha', 0, '')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1, "multi-format must not duplicate rows");
        assert_eq!(
            books[0].formats,
            vec!["EPUB".to_string(), "M4B".to_string()]
        );
        // EPUB wins the primary-filename tiebreak, matching get_book.
        assert_eq!(books[0].filename, "alpha.epub");
    }

    #[tokio::test]
    async fn search_books_returns_one_row_per_book_with_multi_format() {
        // Same regression in the FTS path.
        let _covers = CoversTempDir::new("search_books_multi");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let id = list_books(&pool, "/lib").await.unwrap()[0].id;
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime)
             VALUES (?, 'M4B', 'alpha', 0, '')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        let results = search_books(&pool, "/lib", "Alpha").await.unwrap();
        assert_eq!(results.len(), 1, "FTS results must not duplicate rows");
        assert_eq!(
            results[0].formats,
            vec!["EPUB".to_string(), "M4B".to_string()]
        );
    }

    #[tokio::test]
    async fn list_books_filters_by_author_join() {
        let _covers = CoversTempDir::new("filter_author");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None),
                indexed("b.epub", Some("B"), &["Pratchett"], &[], None, None),
            ],
        )
        .await
        .unwrap();
        let titles: Vec<String> = sqlx::query_scalar(
            "SELECT b.title FROM books b
             JOIN books_authors_link bal ON bal.book = b.id
             JOIN authors a ON a.id = bal.author
             WHERE a.name = ?",
        )
        .bind("Tolkien")
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(titles, vec!["A".to_string()]);
    }

    /// Regression guard for the N+1 fix in list_books / search_books.
    /// Both functions must return all creators, subjects, and identifiers
    /// via the inline json_group_array subqueries rather than per-book
    /// round-trip calls.
    #[tokio::test]
    async fn list_and_search_books_return_multi_valued_fields() {
        let _covers = CoversTempDir::new("multi_valued");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![IndexedBook {
                metadata: EbookMetadata {
                    filename: "multi.epub".into(),
                    title: Some("Multi".into()),
                    creators: vec![
                        Contributor {
                            name: "Alice".into(),
                            ..Default::default()
                        },
                        Contributor {
                            name: "Bob".into(),
                            ..Default::default()
                        },
                    ],
                    subjects: vec!["Fiction".into(), "Sci-Fi".into()],
                    identifiers: vec![
                        Identifier {
                            value: "978-0-000000-00-0".into(),
                            scheme: Some("isbn".into()),
                        },
                        Identifier {
                            value: "https://example.com/book/1".into(),
                            scheme: Some("uri".into()),
                        },
                    ],
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1);
        let book = &books[0];
        assert_eq!(
            book.creators
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alice", "Bob"]
        );
        assert_eq!(
            book.subjects,
            vec!["Fiction".to_string(), "Sci-Fi".to_string()]
        );
        assert_eq!(book.identifiers.len(), 2);
        assert_eq!(book.identifiers[0].scheme.as_deref(), Some("isbn"));
        assert_eq!(book.identifiers[1].scheme.as_deref(), Some("uri"));

        let hits = search_books(&pool, "/lib", "Multi").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(
            hits[0]
                .creators
                .iter()
                .map(|c| c.name.as_str())
                .collect::<Vec<_>>(),
            vec!["Alice", "Bob"]
        );
        assert_eq!(
            hits[0].subjects,
            vec!["Fiction".to_string(), "Sci-Fi".to_string()]
        );
        assert_eq!(hits[0].identifiers.len(), 2);
    }

    #[tokio::test]
    async fn set_settings_prunes_library_when_ebook_path_changes() {
        let _covers = CoversTempDir::new("prune-change");
        let pool = init_db("sqlite::memory:").await.unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/old".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/old",
            vec![indexed(
                "a.epub",
                Some("Dracula"),
                &["Stoker"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        assert_eq!(list_books(&pool, "/old").await.unwrap().len(), 1);

        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/new".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();

        assert!(list_books(&pool, "/old").await.unwrap().is_empty());
        let library_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM libraries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(library_count, 0);
        let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(book_count, 0);
        let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fts_count, 0);
    }

    #[tokio::test]
    async fn set_settings_keeps_libraries_still_configured() {
        let _covers = CoversTempDir::new("prune-keep");
        let pool = init_db("sqlite::memory:").await.unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/books".into()),
                audiobook_library_path: Some("/audio".into()),
            },
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/books",
            vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
        )
        .await
        .unwrap();

        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/books".into()),
                audiobook_library_path: Some("/audio".into()),
            },
        )
        .await
        .unwrap();

        assert_eq!(list_books(&pool, "/books").await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn set_settings_none_removes_library_data() {
        let _covers = CoversTempDir::new("prune-clear");
        let pool = init_db("sqlite::memory:").await.unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/books".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/books",
            vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
        )
        .await
        .unwrap();

        set_settings(
            &pool,
            &Settings {
                ebook_library_path: None,
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();

        let library_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM libraries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(library_count, 0);
    }

    /// Exercises the batched (#149) `prune_orphan_libraries` path across more
    /// than one chunk. The IN-list is chunked at 500 ids to stay under
    /// SQLite's bind-parameter cap, so seeding more orphaned libraries than a
    /// single chunk holds is what actually verifies the chunking loop iterates
    /// (a regression that dropped or mis-bound a later chunk would otherwise go
    /// undetected). Seeds 1001 libraries — three chunks of 500 / 500 / 1 — each
    /// with a book, then prunes them all in one transaction and asserts every
    /// cover UUID is collected (so the lookup is verified to span all chunks)
    /// and every row is gone. Only a handful of rows get an on-disk cover:
    /// materializing 1001 files would be slow, and cover deletion is exercised
    /// by the tracked subset, which straddles the chunk boundaries.
    #[tokio::test]
    async fn prune_orphan_libraries_batches_across_many_libraries() {
        // > 2 full chunks of 500 → the chunk loop runs three times.
        const LIBRARY_COUNT: usize = 1001;
        // Indices spanning every chunk: first row, the first row of the second
        // chunk, a mid-chunk row, and the final (third-chunk) row.
        const MATERIALIZED_COVER_INDICES: [usize; 4] = [0, 500, 750, LIBRARY_COUNT - 1];

        let _covers = CoversTempDir::new("prune-batch");
        let pool = init_db("sqlite::memory:").await.unwrap();

        // Seed the orphaned libraries directly. `keep = []` below marks all of
        // them as orphans regardless of path.
        let mut expected_uuids: Vec<String> = Vec::with_capacity(LIBRARY_COUNT);
        let mut materialized_uuids: Vec<String> = Vec::new();
        for i in 0..LIBRARY_COUNT {
            let path = format!("/orphan-{i}");
            let library_id: i64 = sqlx::query_scalar(
                "INSERT INTO libraries (path, display_name) VALUES (?, ?) RETURNING id",
            )
            .bind(&path)
            .bind(format!("Orphan {i}"))
            .fetch_one(&pool)
            .await
            .unwrap();

            let uuid = format!("uuid-{i}");
            sqlx::query(
                "INSERT INTO books (uuid, library_id, path, title, has_cover)
                 VALUES (?, ?, ?, ?, 1)",
            )
            .bind(&uuid)
            .bind(library_id)
            .bind(format!("{path}/book.epub"))
            .bind(format!("Book {i}"))
            .execute(&pool)
            .await
            .unwrap();

            // Materialize an on-disk cover for a few rows spanning the chunk
            // boundaries so deletion is observable without writing 1001 files.
            if MATERIALIZED_COVER_INDICES.contains(&i) {
                write_cover_file(&uuid, "image/jpeg", b"fake-jpeg").unwrap();
                assert!(cover_path_for(&uuid, "jpg").exists());
                materialized_uuids.push(uuid.clone());
            }

            expected_uuids.push(uuid);
        }

        let mut tx = pool.begin().await.unwrap();
        let mut orphan_uuids = prune_orphan_libraries(&mut tx, &[]).await.unwrap();
        tx.commit().await.unwrap();

        orphan_uuids.sort();
        expected_uuids.sort();
        assert_eq!(
            orphan_uuids, expected_uuids,
            "every orphaned book's cover UUID should be collected across all chunks"
        );

        let library_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM libraries")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(library_count, 0);
        let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(book_count, 0);

        // The caller deletes cover files post-commit; verify the collected
        // UUIDs drive removal of every materialized cover (one per chunk).
        delete_cover_files_for(&orphan_uuids);
        for uuid in &materialized_uuids {
            assert!(
                !cover_path_for(uuid, "jpg").exists(),
                "cover for {uuid} should be deleted"
            );
        }
    }

    #[tokio::test]
    async fn get_book_returns_none_for_missing_id() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let result = get_book(&pool, 9999).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn get_book_is_deterministic_with_multiple_files_and_links() {
        // Regression for PR #55 review: when a book has multiple `book_files`
        // rows (and incidental duplicate publisher/language/series links),
        // get_book() must return the EPUB-preferred filename and stable
        // publisher/language/series values rather than whichever joined row
        // SQLite happens to return first.
        let _covers = CoversTempDir::new("get_book_multi");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha Book"),
                &["Author A"],
                &["Fiction"],
                Some(("Saga", "1")),
                None,
            )],
        )
        .await
        .unwrap();
        let books = list_books(&pool, "/lib").await.unwrap();
        let id = books[0].id;

        // Add a second physical file in another format.
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime)
             VALUES (?, 'M4B', 'alpha', 0, '')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        // Add a second publisher and a second language to exercise the
        // multi-row JOIN path on those link tables. Series already has one
        // row from `replace_books`.
        sqlx::query("INSERT INTO publishers (name) VALUES ('Acme'), ('Zenith')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO books_publishers_link (book, publisher)
             SELECT ?, id FROM publishers WHERE name IN ('Acme', 'Zenith')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query("INSERT INTO languages (code) VALUES ('eng'), ('fra')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO books_languages_link (book, language)
             SELECT ?, id FROM languages WHERE code IN ('eng', 'fra')",
        )
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();

        // Run get_book repeatedly — every call must return identical values.
        let first = get_book(&pool, id).await.unwrap().expect("book");
        for _ in 0..3 {
            let again = get_book(&pool, id).await.unwrap().expect("book");
            assert_eq!(again.filename, first.filename);
            assert_eq!(again.publisher, first.publisher);
            assert_eq!(again.language, first.language);
            assert_eq!(again.series, first.series);
            assert_eq!(again.formats, first.formats);
        }

        // EPUB should win the tiebreak for filename.
        assert_eq!(first.filename, "alpha.epub");
        // Both formats surface in the formats list, sorted by format code.
        assert_eq!(first.formats, vec!["EPUB".to_string(), "M4B".to_string()]);
        // Publisher/language pick alphabetical winners deterministically.
        assert_eq!(first.publisher.as_deref(), Some("Acme"));
        assert_eq!(first.language.as_deref(), Some("eng"));
        assert_eq!(first.series.as_deref(), Some("Saga"));
        assert_eq!(first.series_index.as_deref(), Some("1"));
    }

    #[tokio::test]
    async fn get_book_returns_metadata_for_indexed_book() {
        let _covers = CoversTempDir::new("get_book");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha Book"),
                &["Author A"],
                &["Fiction"],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let books = list_books(&pool, "/lib").await.unwrap();
        let id = books[0].id;

        let book = get_book(&pool, id)
            .await
            .unwrap()
            .expect("book should exist");
        assert_eq!(book.id, id);
        assert_eq!(book.title.as_deref(), Some("Alpha Book"));
        assert_eq!(book.creators.len(), 1);
        assert_eq!(book.creators[0].name, "Author A");
        assert_eq!(book.subjects, vec!["Fiction"]);
        assert!(!book.formats.is_empty(), "formats should be populated");
        assert!(
            book.formats.iter().any(|f| f.eq_ignore_ascii_case("epub")),
            "EPUB format should be present"
        );
    }

    #[tokio::test]
    async fn get_book_handles_book_with_no_relations() {
        // A `books` row that has zero m2m link rows and zero files: every
        // `json_group_array` subquery returns "[]" (over zero inner rows) and
        // every scalar subquery returns NULL. The function must still return
        // a populated `EbookMetadata` with empty vecs and an empty filename
        // rather than erroring out on the missing data.
        let pool = init_db("sqlite::memory:").await.unwrap();
        let lib_res =
            sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
                .execute(&pool)
                .await
                .unwrap();
        let lib_id = lib_res.last_insert_rowid();
        let res = sqlx::query(
            "INSERT INTO books (uuid, library_id, path, title) \
             VALUES ('lonely-uuid', ?, '/lib/lonely', 'Lonely')",
        )
        .bind(lib_id)
        .execute(&pool)
        .await
        .unwrap();
        let id = res.last_insert_rowid();

        let book = get_book(&pool, id)
            .await
            .unwrap()
            .expect("book should exist");
        assert_eq!(book.id, id);
        assert_eq!(book.title.as_deref(), Some("Lonely"));
        assert_eq!(book.filename, "");
        assert!(book.creators.is_empty());
        assert!(book.subjects.is_empty());
        assert!(book.identifiers.is_empty());
        assert!(book.formats.is_empty());
        assert!(book.publisher.is_none());
        assert!(book.language.is_none());
        assert!(book.series.is_none());
        assert!(book.cover_url.is_none());
    }

    #[tokio::test]
    async fn get_book_round_trips_values_containing_control_chars_and_quotes() {
        // Regression for PR #65 review: prior delimiter-based encoding
        // (`GROUP_CONCAT` with 0x1F/0x1E separators) would have silently
        // corrupted any value containing those control chars. The JSON
        // encoding must survive arbitrary UTF-8 — control chars, quotes,
        // backslashes, commas — without altering the round-tripped value.
        let _covers = CoversTempDir::new("get_book_collide");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let nasty_name = "Smith\u{001F}, John \"O'Reilly\" \\back\u{001E}/";
        let nasty_tag = "Sci-Fi\u{001F}Drama";
        let nasty_value = "9780\u{001E}123\"456\\";
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alpha.epub",
                Some("Alpha"),
                &[nasty_name],
                &[nasty_tag],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let id = list_books(&pool, "/lib").await.unwrap()[0].id;
        sqlx::query("INSERT INTO book_identifiers (book_id, scheme, value) VALUES (?, 'ISBN', ?)")
            .bind(id)
            .bind(nasty_value)
            .execute(&pool)
            .await
            .unwrap();

        let book = get_book(&pool, id).await.unwrap().expect("book");
        assert_eq!(book.creators.len(), 1);
        assert_eq!(book.creators[0].name, nasty_name);
        assert_eq!(book.subjects, vec![nasty_tag.to_string()]);
        assert_eq!(book.identifiers.len(), 1);
        assert_eq!(book.identifiers[0].value, nasty_value);
        assert_eq!(book.identifiers[0].scheme.as_deref(), Some("ISBN"));
    }

    #[test]
    fn sanitize_description_preserves_safe_html() {
        let cleaned = sanitize_description(Some(
            "<p>Hello <strong>world</strong>!</p><p>Second <em>line</em>.</p>".into(),
        ))
        .unwrap();
        assert!(cleaned.contains("<p>"));
        assert!(cleaned.contains("<strong>world</strong>"));
        assert!(cleaned.contains("<em>line</em>"));
    }

    #[test]
    fn sanitize_description_strips_scripts_and_event_handlers() {
        // ammonia's defaults must drop <script>, inline `onerror`, and
        // `javascript:` URLs. Anything that could execute on the detail page
        // when rendered via dangerous_inner_html is the threat model here.
        let cleaned = sanitize_description(Some(
            "<p>Safe</p><script>alert('xss')</script>\
             <img src=x onerror=\"alert(1)\"/>\
             <a href=\"javascript:alert(1)\">click</a>"
                .into(),
        ))
        .unwrap();
        assert!(!cleaned.contains("<script"));
        assert!(!cleaned.to_ascii_lowercase().contains("onerror"));
        assert!(!cleaned.to_ascii_lowercase().contains("javascript:"));
        assert!(cleaned.contains("<p>Safe</p>"));
    }

    #[test]
    fn sanitize_description_collapses_empty_input_to_none() {
        assert_eq!(sanitize_description(None), None);
        assert_eq!(sanitize_description(Some(String::new())), None);
        assert_eq!(sanitize_description(Some("   \n\t".into())), None);
        // A bare <script> with no other content sanitizes to "" and must
        // collapse to None so the UI hides the description block entirely.
        assert_eq!(
            sanitize_description(Some("<script>alert(1)</script>".into())),
            None
        );
    }

    #[tokio::test]
    async fn get_book_returns_sanitized_html_description() {
        let _covers = CoversTempDir::new("sanitize_desc");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let raw =
            "<p>Brief.</p><script>alert('xss')</script><p>More <b>detail</b>.</p>".to_string();
        replace_books(
            &pool,
            "/lib",
            vec![IndexedBook {
                metadata: EbookMetadata {
                    filename: "alpha.epub".into(),
                    title: Some("Alpha".into()),
                    description: Some(raw),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();
        let id = list_books(&pool, "/lib").await.unwrap()[0].id;

        let desc = get_book(&pool, id).await.unwrap().unwrap().description;
        let desc = desc.expect("description should be present");
        assert!(desc.contains("<p>Brief.</p>"));
        assert!(desc.contains("<b>detail</b>"));
        assert!(!desc.contains("<script"));
    }

    // -------------------------------------------------------------------------
    // Discovery query tests (F1.8)
    // -------------------------------------------------------------------------

    /// Seed a small multi-author, multi-series, multi-tag fixture for the
    /// discovery query tests below. Returns the pool and a `CoversTempDir`
    /// guard the caller must keep alive for the lifetime of the test.
    async fn seed_discovery_fixture() -> (SqlitePool, CoversTempDir) {
        let guard = CoversTempDir::new("discovery");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                // Two-author book in Saga #1 with tag "fiction"
                indexed(
                    "saga1.epub",
                    Some("Saga: Book One"),
                    &["Ada Lovelace", "Grace Hopper"],
                    &["fiction", "classic"],
                    Some(("Saga", "1")),
                    None,
                ),
                // Sequel in Saga #2, same primary author + new tag
                indexed(
                    "saga2.epub",
                    Some("Saga: Book Two"),
                    &["Ada Lovelace"],
                    &["fiction"],
                    Some(("Saga", "2")),
                    None,
                ),
                // Standalone by Ada — no series
                indexed(
                    "standalone.epub",
                    Some("Standalone"),
                    &["Ada Lovelace"],
                    &["essay"],
                    None,
                    None,
                ),
                // Different-author, different-series book
                indexed(
                    "other.epub",
                    Some("Other Story"),
                    &["Niklaus Wirth"],
                    &["nonfiction"],
                    Some(("Pioneers", "1")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();
        (pool, guard)
    }

    async fn author_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT id FROM authors WHERE name = ?")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    async fn series_id_by_name(pool: &SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar::<_, i64>("SELECT id FROM series WHERE name = ?")
            .bind(name)
            .fetch_one(pool)
            .await
            .unwrap()
    }

    #[tokio::test]
    async fn get_author_returns_author_with_all_books_ordered_by_series_index() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let id = author_id_by_name(&pool, "Ada Lovelace").await;

        let author = get_author(&pool, id).await.unwrap().expect("author exists");

        assert_eq!(author.name, "Ada Lovelace");
        assert_eq!(author.book_count, 3);
        assert_eq!(author.books.len(), 3);

        // Series books come first, ordered by series_index ASC (NULLS LAST
        // means the standalone trails).
        let titles: Vec<_> = author
            .books
            .iter()
            .filter_map(|b| b.title.clone())
            .collect();
        assert_eq!(
            titles,
            vec![
                "Saga: Book One".to_string(),
                "Saga: Book Two".to_string(),
                "Standalone".to_string(),
            ]
        );
    }

    #[tokio::test]
    async fn get_author_populates_series_id_on_books() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let id = author_id_by_name(&pool, "Ada Lovelace").await;
        let expected_sid = series_id_by_name(&pool, "Saga").await;

        let author = get_author(&pool, id).await.unwrap().unwrap();
        for book in author.books.iter().filter(|b| b.series.is_some()) {
            assert_eq!(
                book.series_id,
                Some(expected_sid),
                "series book should carry series_id"
            );
        }
        let standalone = author
            .books
            .iter()
            .find(|b| b.series.is_none())
            .expect("standalone present");
        assert_eq!(standalone.series_id, None);
    }

    #[tokio::test]
    async fn get_author_returns_none_for_missing_id() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let missing = get_author(&pool, 999_999).await.unwrap();
        assert!(missing.is_none());
    }

    #[tokio::test]
    async fn get_series_returns_books_ordered_by_series_index() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let id = series_id_by_name(&pool, "Saga").await;

        let series = get_series(&pool, id).await.unwrap().expect("series exists");
        assert_eq!(series.name, "Saga");
        assert_eq!(series.book_count, 2);

        let titles: Vec<_> = series
            .books
            .iter()
            .filter_map(|b| b.title.clone())
            .collect();
        assert_eq!(
            titles,
            vec!["Saga: Book One".to_string(), "Saga: Book Two".to_string()]
        );
        // Each book should carry the parent series id back out so the
        // frontend can navigate cross-references without an extra lookup.
        for book in &series.books {
            assert_eq!(book.series_id, Some(id));
        }
    }

    #[tokio::test]
    async fn get_series_returns_none_for_missing_id() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let missing = get_series(&pool, 999_999).await.unwrap();
        assert!(missing.is_none());
    }

    // -------------------------------------------------------------------------
    // Discovery read caps (issue #150)
    //
    // `get_author` / `get_series` previously serialized every attributed book
    // in one payload. The fix is a hard `LIMIT MAX_DISCOVERY_BOOKS` on the
    // nested `books` vec plus an uncapped `book_count` so callers can detect
    // truncation as `book_count > books.len()`.
    // -------------------------------------------------------------------------

    /// Seed `count` minimal `books` rows under `/lib`, all linked to one
    /// author ("Prolific") and one series ("Mega"), via recursive CTEs.
    /// Bypasses `replace_books`/the indexer — the cap only depends on link
    /// rows existing — keeping the test fast even past the 1k cap. Returns
    /// `(author_id, series_id)`.
    async fn seed_books_for_one_author_and_series(pool: &SqlitePool, count: i64) -> (i64, i64) {
        sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
            .execute(pool)
            .await
            .unwrap();
        let lib_id: i64 = sqlx::query_scalar("SELECT id FROM libraries WHERE path = '/lib'")
            .fetch_one(pool)
            .await
            .unwrap();
        let author_id: i64 = sqlx::query_scalar(
            "INSERT INTO authors (name, sort) VALUES ('Prolific', 'Prolific') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        let series_id: i64 = sqlx::query_scalar(
            "INSERT INTO series (name, sort) VALUES ('Mega', 'Mega') RETURNING id",
        )
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            WITH RECURSIVE n(i) AS (
                SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?
            )
            INSERT INTO books (uuid, library_id, path, title, sort, series_index)
            SELECT 'uuid-' || i, ?, '/lib/b' || i, 'Title ' || i,
                   'Title ' || printf('%010d', i), i
              FROM n
            "#,
        )
        .bind(count)
        .bind(lib_id)
        .execute(pool)
        .await
        .unwrap();
        // Link every seeded book to the author and the series.
        sqlx::query(
            "INSERT INTO books_authors_link (book, author, position)
             SELECT id, ?, 0 FROM books",
        )
        .bind(author_id)
        .execute(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO books_series_link (book, series)
             SELECT id, ? FROM books",
        )
        .bind(series_id)
        .execute(pool)
        .await
        .unwrap();
        (author_id, series_id)
    }

    #[tokio::test]
    async fn get_author_caps_books_at_max_discovery_books() {
        let _covers = CoversTempDir::new("author_cap");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let total = MAX_DISCOVERY_BOOKS + 25;
        let (author_id, _series_id) = seed_books_for_one_author_and_series(&pool, total).await;

        let author = get_author(&pool, author_id)
            .await
            .unwrap()
            .expect("author exists");
        assert_eq!(
            author.books.len() as i64,
            MAX_DISCOVERY_BOOKS,
            "get_author must cap the nested books vec at MAX_DISCOVERY_BOOKS"
        );
        assert_eq!(
            author.book_count as i64, total,
            "book_count must report the true (uncapped) shelf size"
        );
        assert!(
            author.book_count > author.books.len(),
            "truncation must be detectable as book_count > books.len()"
        );
    }

    #[tokio::test]
    async fn get_series_caps_books_at_max_discovery_books() {
        let _covers = CoversTempDir::new("series_cap");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let total = MAX_DISCOVERY_BOOKS + 25;
        let (_author_id, series_id) = seed_books_for_one_author_and_series(&pool, total).await;

        let series = get_series(&pool, series_id)
            .await
            .unwrap()
            .expect("series exists");
        assert_eq!(
            series.books.len() as i64,
            MAX_DISCOVERY_BOOKS,
            "get_series must cap the nested books vec at MAX_DISCOVERY_BOOKS"
        );
        assert_eq!(
            series.book_count as i64, total,
            "book_count must report the true (uncapped) series size"
        );
        assert!(
            series.book_count > series.books.len(),
            "truncation must be detectable as book_count > books.len()"
        );
    }

    #[tokio::test]
    async fn get_tag_cloud_returns_counts_ordered_by_count_then_name() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let tags = get_tag_cloud(&pool).await.unwrap();

        // Fixture has: fiction × 2, classic × 1, essay × 1, nonfiction × 1.
        // Order: cnt DESC, then name ASC.
        let names: Vec<_> = tags.iter().map(|t| t.name.clone()).collect();
        assert_eq!(
            names,
            vec![
                "fiction".to_string(),
                "classic".to_string(),
                "essay".to_string(),
                "nonfiction".to_string(),
            ]
        );
        assert_eq!(tags[0].count, 2);
        assert!(tags[1..].iter().all(|t| t.count == 1));
    }

    #[tokio::test]
    async fn get_tag_cloud_returns_empty_vec_when_no_tags() {
        let _guard = CoversTempDir::new("empty_tags");
        let pool = init_db("sqlite::memory:").await.unwrap();
        // No books, no tags.
        let tags = get_tag_cloud(&pool).await.unwrap();
        assert!(tags.is_empty());
    }

    #[tokio::test]
    async fn get_tag_cloud_counts_reflect_overrides() {
        // F5.1: per-tag counts in the cloud must follow the merged view.
        // Without the override-aware count, the cloud kept showing the
        // canonical totals — over-reporting tags the user had removed
        // from books and missing books whose tags were reassigned via
        // override.
        let _guard = CoversTempDir::new("tag_cloud_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["X"], &["fiction"], None, None),
                indexed("b.epub", Some("B"), &["X"], &["fiction"], None, None),
                indexed("c.epub", Some("C"), &["X"], &["essay"], None, None),
            ],
        )
        .await
        .unwrap();

        // Sanity: canonical counts before any overrides.
        let pre = get_tag_cloud(&pool).await.unwrap();
        let fiction_pre = pre
            .iter()
            .find(|t| t.name == "fiction")
            .expect("fiction present pre-override");
        assert_eq!(fiction_pre.count, 2);

        // Reassign a.epub: drop "fiction", add "essay".
        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books.iter().find(|b| b.filename == "a.epub").unwrap();
        let uuid = a.unique_identifier.clone().unwrap();
        let ov = MetadataOverrides {
            subjects: Some(vec!["essay".into()]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let post = get_tag_cloud(&pool).await.unwrap();
        let fiction = post
            .iter()
            .find(|t| t.name == "fiction")
            .expect("fiction still visible (canonical anchor remains on b.epub)");
        assert_eq!(
            fiction.count, 1,
            "fiction should drop a.epub after override, got {post:?}",
        );
        let essay = post
            .iter()
            .find(|t| t.name == "essay")
            .expect("essay present");
        assert_eq!(
            essay.count, 2,
            "essay should pick up override-tagged a.epub, got {post:?}",
        );
    }

    // -----------------------------------------------------------------
    // F1.12 index pages — list_authors / list_series
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn list_authors_returns_all_with_counts_and_alpha_order() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let authors = list_authors(&pool, "/lib").await.unwrap();

        // Three distinct authors: Ada Lovelace, Grace Hopper, Niklaus Wirth.
        let names: Vec<_> = authors.iter().map(|a| a.name.clone()).collect();
        assert_eq!(
            names,
            vec![
                "Ada Lovelace".to_string(),
                "Grace Hopper".to_string(),
                "Niklaus Wirth".to_string(),
            ],
            "expected NOCASE alphabetical order by sort/name"
        );

        // Book counts: Ada=3, Grace=1, Niklaus=1.
        let by_name: std::collections::HashMap<_, _> = authors
            .iter()
            .map(|a| (a.name.clone(), a.book_count))
            .collect();
        assert_eq!(by_name["Ada Lovelace"], 3);
        assert_eq!(by_name["Grace Hopper"], 1);
        assert_eq!(by_name["Niklaus Wirth"], 1);

        // IDs are populated so cards can route to /authors/:id.
        assert!(authors.iter().all(|a| a.id > 0));
    }

    #[tokio::test]
    async fn list_authors_scopes_to_library_path() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let authors = list_authors(&pool, "/no-such-library").await.unwrap();
        assert!(
            authors.is_empty(),
            "unknown library path must yield empty list"
        );
    }

    #[tokio::test]
    async fn list_authors_returns_empty_for_empty_library() {
        let _guard = CoversTempDir::new("empty_authors");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let authors = list_authors(&pool, "/lib").await.unwrap();
        assert!(authors.is_empty());
    }

    #[tokio::test]
    async fn list_series_returns_all_with_counts_and_alpha_order() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let series = list_series(&pool, "/lib").await.unwrap();

        // Two series: Pioneers, Saga (NOCASE alpha).
        let names: Vec<_> = series.iter().map(|s| s.name.clone()).collect();
        assert_eq!(names, vec!["Pioneers".to_string(), "Saga".to_string()]);

        let by_name: std::collections::HashMap<_, _> = series
            .iter()
            .map(|s| (s.name.clone(), s.book_count))
            .collect();
        assert_eq!(by_name["Saga"], 2);
        assert_eq!(by_name["Pioneers"], 1);
    }

    #[tokio::test]
    async fn list_series_populates_primary_author_from_first_book() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let series = list_series(&pool, "/lib").await.unwrap();

        let by_name: std::collections::HashMap<_, _> = series
            .iter()
            .map(|s| (s.name.clone(), s.primary_author.clone()))
            .collect();
        // Saga book one's first creator is "Ada Lovelace" (the two-author
        // book lists Ada first); Pioneers has Niklaus Wirth as sole author.
        assert_eq!(by_name["Saga"], Some("Ada Lovelace".to_string()));
        assert_eq!(by_name["Pioneers"], Some("Niklaus Wirth".to_string()));
    }

    #[tokio::test]
    async fn list_series_scopes_to_library_path() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let series = list_series(&pool, "/no-such-library").await.unwrap();
        assert!(series.is_empty());
    }

    // F5.1 — index-page counts must follow the same override overlay
    // applied to `/authors/:id` and `/series/:id` in PR #153. Without
    // these, an author whose books were reassigned through the edit
    // form still reports the canonical count on /authors, then
    // /authors/:id shows the corrected list — a visible inconsistency.

    #[tokio::test]
    async fn list_authors_book_count_follows_override_creators() {
        // Setup: Ada has 3 canonical books, Grace has 1 (saga1 lists
        // both, with Ada first). Override saga2 so its single creator
        // is Grace instead of Ada. Expected effective counts:
        //   Ada   → 2 (saga1 + standalone; saga2 reassigned away)
        //   Grace → 2 (saga1 still + saga2 from override)
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let books = list_books(&pool, "/lib").await.unwrap();
        let saga2 = books.iter().find(|b| b.filename == "saga2.epub").unwrap();
        let uuid = saga2.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "Grace Hopper".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let authors = list_authors(&pool, "/lib").await.unwrap();
        let by_name: std::collections::HashMap<_, _> = authors
            .iter()
            .map(|a| (a.name.clone(), a.book_count))
            .collect();
        assert_eq!(
            by_name.get("Ada Lovelace").copied(),
            Some(2),
            "Ada loses saga2 to the override",
        );
        assert_eq!(
            by_name.get("Grace Hopper").copied(),
            Some(2),
            "Grace picks up saga2 from the override",
        );
    }

    #[tokio::test]
    async fn list_authors_book_count_matches_canonical_creator_case_insensitively() {
        // `authors.name` is `UNIQUE COLLATE NOCASE`; an override that
        // differs only by case ("ada lovelace") still resolves to the
        // canonical "Ada Lovelace" row. Mirrors the NOCASE follow-up
        // applied to /author/:id (commit aca8a81b).
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let books = list_books(&pool, "/lib").await.unwrap();
        let other = books.iter().find(|b| b.filename == "other.epub").unwrap();
        let uuid = other.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "ADA LOVELACE".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let authors = list_authors(&pool, "/lib").await.unwrap();
        let ada = authors
            .iter()
            .find(|a| a.name == "Ada Lovelace")
            .expect("Ada present");
        assert_eq!(
            ada.book_count, 4,
            "case-mismatched override should still increment canonical Ada's count",
        );
    }

    #[tokio::test]
    async fn list_series_book_count_follows_override_series() {
        // Move the standalone book into Saga via override. Expected:
        // Saga's count goes from 2 → 3. (The canonical books_series_link
        // is untouched; the overlay surfaces the effective membership.)
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some("3".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = list_series(&pool, "/lib").await.unwrap();
        let saga = series
            .iter()
            .find(|s| s.name == "Saga")
            .expect("Saga present");
        assert_eq!(saga.book_count, 3, "override should add standalone to Saga");
    }

    #[tokio::test]
    async fn list_series_primary_author_follows_override_creators() {
        // Saga's first book (saga1.epub by series_index) has canonical
        // creators [Ada Lovelace, Grace Hopper] — primary_author is
        // "Ada Lovelace". Override the first creator to "Margaret
        // Hamilton"; the index by-line should follow.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let books = list_books(&pool, "/lib").await.unwrap();
        let saga1 = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga1.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "Margaret Hamilton".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = list_series(&pool, "/lib").await.unwrap();
        let saga = series
            .iter()
            .find(|s| s.name == "Saga")
            .expect("Saga present");
        assert_eq!(
            saga.primary_author.as_deref(),
            Some("Margaret Hamilton"),
            "override creator drives the index by-line",
        );
    }

    // -----------------------------------------------------------------
    // F5.1 Metadata overrides
    // -----------------------------------------------------------------

    #[tokio::test]
    async fn upsert_and_get_metadata_overrides_roundtrips() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        // Create a user for updated_by.
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        let ov = MetadataOverrides {
            title: Some("New Title".into()),
            description: Some("A new description".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, "test-uuid-1", &ov, false, user_id)
            .await
            .unwrap();

        let (loaded, has_cover) = get_metadata_overrides(&pool, "test-uuid-1")
            .await
            .unwrap()
            .expect("overrides should exist");
        assert_eq!(loaded.title, Some("New Title".into()));
        assert_eq!(loaded.description, Some("A new description".into()));
        assert_eq!(loaded.publisher, None);
        assert!(!has_cover);
    }

    #[tokio::test]
    async fn merge_metadata_overrides_accumulates_fields_and_preserves_cover_flag() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Seed an existing override carrying a title AND a user-uploaded cover.
        let initial = MetadataOverrides {
            title: Some("First Title".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, "merge-uuid", &initial, true, user_id)
            .await
            .unwrap();

        // A later edit touching only `description` must not clobber the title
        // (the incremental-edit contract the TOCTOU race nullified) and must
        // not reset the cover flag (the pre-#166 reset bug).
        let edit = MetadataOverrides {
            description: Some("Added later".into()),
            ..Default::default()
        };
        merge_metadata_overrides(&pool, "merge-uuid", &edit, user_id)
            .await
            .unwrap();

        let (loaded, has_cover) = get_metadata_overrides(&pool, "merge-uuid")
            .await
            .unwrap()
            .expect("overrides should exist");
        assert_eq!(
            loaded.title,
            Some("First Title".into()),
            "prior title must survive a description-only merge"
        );
        assert_eq!(loaded.description, Some("Added later".into()));
        assert!(
            has_cover,
            "has_cover_override must carry forward across a text-only merge"
        );
    }

    #[tokio::test]
    async fn merge_metadata_overrides_creates_row_when_absent() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let edit = MetadataOverrides {
            title: Some("Fresh".into()),
            ..Default::default()
        };
        merge_metadata_overrides(&pool, "fresh-uuid", &edit, user_id)
            .await
            .unwrap();
        let (loaded, has_cover) = get_metadata_overrides(&pool, "fresh-uuid")
            .await
            .unwrap()
            .expect("overrides should exist");
        assert_eq!(loaded.title, Some("Fresh".into()));
        assert!(!has_cover, "a brand-new merged row has no cover override");
    }

    #[tokio::test]
    async fn get_metadata_overrides_returns_none_when_absent() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let result = get_metadata_overrides(&pool, "nonexistent-uuid")
            .await
            .unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn delete_metadata_overrides_removes_row() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ov = MetadataOverrides {
            title: Some("Override".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, "del-uuid", &ov, false, user_id)
            .await
            .unwrap();
        assert!(get_metadata_overrides(&pool, "del-uuid")
            .await
            .unwrap()
            .is_some());

        delete_metadata_overrides(&pool, "del-uuid").await.unwrap();
        assert!(get_metadata_overrides(&pool, "del-uuid")
            .await
            .unwrap()
            .is_none());
    }

    /// Bug #1: saving a title override must rebuild `books_fts` so search
    /// finds the new title and stops matching the original one.
    #[tokio::test]
    async fn upsert_metadata_overrides_rebuilds_fts_for_title() {
        let _covers = CoversTempDir::new("fts_override_title");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Original Title"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        // Sanity: search finds the original title.
        let hits = search_books(&pool, "/lib", "Original").await.unwrap();
        assert_eq!(hits.len(), 1);

        // Save an override that changes the title.
        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        let ov = MetadataOverrides {
            title: Some("Brand New Title".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // Search now matches the overridden title and no longer the original.
        let new_hits = search_books(&pool, "/lib", "Brand").await.unwrap();
        assert_eq!(new_hits.len(), 1);
        assert_eq!(new_hits[0].title.as_deref(), Some("Brand New Title"));
        let old_hits = search_books(&pool, "/lib", "Original").await.unwrap();
        assert!(
            old_hits.is_empty(),
            "FTS still matches the pre-override title"
        );
    }

    /// Bug #1: the palette uses the same `books_fts` table, so the override
    /// rebuild must also surface there.
    #[tokio::test]
    async fn upsert_metadata_overrides_rebuilds_fts_for_palette() {
        let _covers = CoversTempDir::new("fts_override_palette");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Scanned Title"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        let ov = MetadataOverrides {
            title: Some("Edited Palette Title".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let palette = search_palette(&pool, "/lib", "Edited").await.unwrap();
        assert_eq!(palette.books.len(), 1);
    }

    /// Bug #1 follow-on: deleting the override should restore the FTS row
    /// to the canonical scanned values.
    #[tokio::test]
    async fn delete_metadata_overrides_restores_fts() {
        let _covers = CoversTempDir::new("fts_override_revert");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "r.epub",
                Some("Canonical Title"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();

        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                title: Some("Temporary Override".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();
        assert_eq!(
            search_books(&pool, "/lib", "Temporary")
                .await
                .unwrap()
                .len(),
            1
        );

        delete_metadata_overrides(&pool, &uuid).await.unwrap();

        // FTS is back to the canonical title; the override token no longer
        // matches.
        assert_eq!(
            search_books(&pool, "/lib", "Canonical")
                .await
                .unwrap()
                .len(),
            1
        );
        assert!(search_books(&pool, "/lib", "Temporary")
            .await
            .unwrap()
            .is_empty());
    }

    #[tokio::test]
    async fn get_book_merges_scalar_overrides() {
        let _covers = CoversTempDir::new("merge_scalar");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "merge.epub",
                Some("Original Title"),
                &["Author A"],
                &["fiction"],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let book = &books[0];
        let uuid = book.unique_identifier.clone().unwrap();
        let id = book.id;

        // Save overrides.
        let ov = MetadataOverrides {
            title: Some("Edited Title".into()),
            publisher: Some("New Publisher".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // get_book should return merged values.
        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(merged.title.as_deref(), Some("Edited Title"));
        assert_eq!(merged.publisher.as_deref(), Some("New Publisher"));
        assert!(merged.has_override);
        // Non-overridden fields unchanged.
        assert_eq!(merged.creators[0].name, "Author A");
    }

    #[tokio::test]
    async fn get_book_merges_creators_replaces_entirely() {
        let _covers = CoversTempDir::new("merge_creators");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "creators.epub",
                Some("Book"),
                &["Author A"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        let id = books[0].id;

        let ov = MetadataOverrides {
            creators: Some(vec![
                Contributor {
                    name: "Author B".into(),
                    ..Default::default()
                },
                Contributor {
                    name: "Author C".into(),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(merged.creators.len(), 2);
        assert_eq!(merged.creators[0].name, "Author B");
        assert_eq!(merged.creators[1].name, "Author C");
    }

    #[tokio::test]
    async fn get_book_merges_subjects_replaces_entirely() {
        let _covers = CoversTempDir::new("merge_subjects");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "subjects.epub",
                Some("Book"),
                &["Author"],
                &["fiction", "classic"],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        let id = books[0].id;

        let ov = MetadataOverrides {
            subjects: Some(vec!["sci-fi".into(), "adventure".into()]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(merged.subjects, vec!["sci-fi", "adventure"]);
    }

    #[tokio::test]
    async fn get_book_backfills_creator_ids_after_override_replaces_authors() {
        // Override Contributors carry only a name, so a book whose author
        // list was edited through the metadata form would otherwise come
        // back with `creators[*].id == None`, rendering the breadcrumb's
        // author link as an unclickable span even when the `authors` row
        // exists. Verify get_book backfills the id by name. Mirrors the
        // user's report against book 268 (multi-author book where the
        // user removed all but one canonical author).
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        // saga1.epub canonically has ["Ada Lovelace", "Grace Hopper"];
        // simulate the user dropping the second author through the edit
        // form. apply_overrides replaces creators wholesale, so the
        // override Contributor has id = None.
        let books = list_books(&pool, "/lib").await.unwrap();
        let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga_one.unique_identifier.clone().unwrap();
        let book_id = saga_one.id;

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "Ada Lovelace".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, book_id).await.unwrap().unwrap();
        assert_eq!(merged.creators.len(), 1);
        assert_eq!(merged.creators[0].name, "Ada Lovelace");
        assert_eq!(
            merged.creators[0].id,
            Some(ada_id),
            "creator id must be backfilled so the breadcrumb renders as a Link",
        );
    }

    #[tokio::test]
    async fn get_book_backfills_creator_ids_case_insensitively() {
        // `authors.name` is `UNIQUE COLLATE NOCASE`, so a SQL `IN (...)`
        // lookup matches case-insensitively — but the returned row carries
        // the DB casing while the override carries the user-supplied
        // casing. The HashMap must normalise both sides to lowercase so
        // an override like "ada lovelace" still resolves to the canonical
        // "Ada Lovelace" id.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga_one.unique_identifier.clone().unwrap();
        let book_id = saga_one.id;

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "ADA LOVELACE".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, book_id).await.unwrap().unwrap();
        assert_eq!(merged.creators.len(), 1);
        assert_eq!(merged.creators[0].name, "ADA LOVELACE");
        assert_eq!(
            merged.creators[0].id,
            Some(ada_id),
            "case-mismatched override should still resolve to the canonical author id",
        );
    }

    #[tokio::test]
    async fn get_book_leaves_creator_id_none_when_override_author_unknown() {
        // If the override sets an author name that doesn't exist in the
        // `authors` table, backfill must leave the id None — same shape
        // as get_book_leaves_series_id_none_when_override_series_unknown.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        let books = list_books(&pool, "/lib").await.unwrap();
        let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga_one.unique_identifier.clone().unwrap();
        let book_id = saga_one.id;

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "Nobody Indexed".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, book_id).await.unwrap().unwrap();
        assert_eq!(merged.creators.len(), 1);
        assert_eq!(merged.creators[0].name, "Nobody Indexed");
        assert_eq!(merged.creators[0].id, None);
    }

    #[tokio::test]
    async fn get_book_backfills_series_id_from_override_when_series_exists() {
        // A book whose series was set via overrides (not at scan time)
        // historically came back with series_id == None even though the
        // series row existed in the relational table. The detail page's
        // "Series" rail then fell back to plain text instead of a Link
        // to /series/:id. Verify the read path now backfills the id.
        let _covers = CoversTempDir::new("override_series_link");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Seed: one book belongs to "Saga" natively (so the series row exists),
        // one standalone book that we'll later override into the same series.
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "saga1.epub",
                    Some("Saga: Book One"),
                    &["Author X"],
                    &[],
                    Some(("Saga", "1")),
                    None,
                ),
                indexed("loner.epub", Some("Loner"), &["Author Y"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let saga_id = series_id_by_name(&pool, "Saga").await;
        let books = list_books(&pool, "/lib").await.unwrap();
        let loner = books.iter().find(|b| b.filename == "loner.epub").unwrap();
        assert_eq!(loner.series, None);
        assert_eq!(loner.series_id, None);
        let loner_uuid = loner.unique_identifier.clone().unwrap();
        let loner_book_id = loner.id;

        // Override the standalone to be part of "Saga". The overrides path
        // does not touch books_series_link, so loner.series_id stays unset
        // in the relational table — get_book must backfill from the series
        // table by name.
        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some("3".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &loner_uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, loner_book_id).await.unwrap().unwrap();
        assert_eq!(merged.series.as_deref(), Some("Saga"));
        assert_eq!(
            merged.series_id,
            Some(saga_id),
            "override-only series must still resolve series_id so the detail rail can link"
        );
    }

    #[tokio::test]
    async fn get_author_includes_books_whose_override_names_this_author() {
        // Repro of the bug where renaming a book's author via the
        // metadata form (e.g. "Sanderson, Brandon" → "Brandon Sanderson")
        // left the book invisible on the new author's `/author/:id` page.
        // The override path writes JSON only — `books_authors_link` keeps
        // pointing at the canonical author row — so `get_author` must
        // layer overrides on top of the relational link at read time.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Set up the "Brandon Sanderson" vs "Sanderson, Brandon" shape:
        // one canonical author and a second name the user prefers, then
        // override one book to use the preferred name.
        let canonical_id = author_id_by_name(&pool, "Ada Lovelace").await;
        let preferred_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id",
        )
        .bind("Lovelace, Ada")
        .bind("Lovelace, Ada")
        .fetch_one(&pool)
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let saga_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = saga_one.unique_identifier.clone().unwrap();
        let saga_one_id = saga_one.id;

        // saga1.epub canonically lists ["Ada Lovelace", "Grace Hopper"];
        // the override renames the primary author to "Lovelace, Ada".
        let ov = MetadataOverrides {
            creators: Some(vec![
                Contributor {
                    name: "Lovelace, Ada".into(),
                    role: Some("aut".into()),
                    file_as: None,
                    id: None,
                },
                Contributor {
                    name: "Grace Hopper".into(),
                    role: Some("aut".into()),
                    file_as: None,
                    id: None,
                },
            ]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // Visiting the preferred-name author page must now include the
        // overridden book, even though `books_authors_link` for that book
        // still points at the canonical "Ada Lovelace" row.
        let preferred = get_author(&pool, preferred_id)
            .await
            .unwrap()
            .expect("author exists");
        let titles: Vec<_> = preferred
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            vec!["Saga: Book One".to_string()],
            "override-named author must surface the book on /author/:id",
        );

        // And the canonical-name author page must drop it, because the
        // override replaced the creator list wholesale.
        let canonical = get_author(&pool, canonical_id)
            .await
            .unwrap()
            .expect("author exists");
        let canonical_titles: Vec<_> = canonical
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert!(
            !canonical_titles.contains(&"Saga: Book One".to_string()),
            "override moved the book off the canonical author, got {canonical_titles:?}",
        );

        // The card on the preferred-name page should show the override
        // creator name, not the canonical one.
        let card = &preferred.books[0];
        assert_eq!(card.id, saga_one_id);
        assert_eq!(
            card.creators.first().map(|c| c.name.as_str()),
            Some("Lovelace, Ada")
        );
    }

    #[tokio::test]
    async fn get_author_excludes_books_whose_override_clears_authors() {
        // A book whose override sets creators to the empty array should
        // disappear from every author's page, matching what the book
        // detail page already shows (no breadcrumb author).
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            creators: Some(vec![]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let ada = get_author(&pool, ada_id)
            .await
            .unwrap()
            .expect("author exists");
        let titles: Vec<_> = ada
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert!(
            !titles.contains(&"Standalone".to_string()),
            "override-cleared creators must drop the book from /author/:id, got {titles:?}",
        );
    }

    #[tokio::test]
    async fn get_author_override_creator_match_is_case_insensitive() {
        // `authors.name` is `UNIQUE COLLATE NOCASE`, so an override that
        // differs only by case from the target author's row must still
        // surface the book on `/author/:id`. The override comparison
        // gets an explicit `COLLATE NOCASE` because the LHS is a
        // `json_extract(...)` expression (BINARY by default) and the RHS
        // is a bound parameter (also no collation).
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        // Override uses lowercase casing; canonical row is "Ada Lovelace".
        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "ada lovelace".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let ada = get_author(&pool, ada_id)
            .await
            .unwrap()
            .expect("author exists");
        let titles: Vec<_> = ada
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert!(
            titles.contains(&"Standalone".to_string()),
            "lowercase override should still match NOCASE author row, got {titles:?}",
        );
    }

    #[tokio::test]
    async fn get_series_includes_books_added_via_override() {
        // Repro of the bug where editing a book to set its series via the
        // metadata form left the book invisible on `/series/:id`. The
        // override path only writes JSON into `metadata_overrides` and
        // never touches `books_series_link`, so `get_series` must layer
        // overrides on top of the relational link at read time.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let saga_id = series_id_by_name(&pool, "Saga").await;

        // Loner has no canonical series at all. After the override it
        // should show up as #3 in Saga, after the two indexed books.
        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let standalone_uuid = standalone.unique_identifier.clone().unwrap();
        let standalone_id = standalone.id;

        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some("3".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &standalone_uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("series exists");
        assert_eq!(series.book_count, 3);

        let titles: Vec<_> = series
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            vec![
                "Saga: Book One".to_string(),
                "Saga: Book Two".to_string(),
                "Standalone".to_string(),
            ],
            "override-set series_index=3 should sort the overridden book last",
        );

        // The overridden book must carry the parent series id so the card
        // links back to /series/:id.
        let overridden = series.books.iter().find(|b| b.id == standalone_id).unwrap();
        assert_eq!(overridden.series_id, Some(saga_id));
        assert_eq!(overridden.series.as_deref(), Some("Saga"));
    }

    #[tokio::test]
    async fn get_series_excludes_books_whose_override_clears_series() {
        // A book canonically in Saga whose override clears the series (sets
        // series to an empty string) should disappear from /series/:id,
        // matching what the book detail page already shows.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let saga_id = series_id_by_name(&pool, "Saga").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let book_two = books.iter().find(|b| b.filename == "saga2.epub").unwrap();
        let uuid = book_two.unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            series: Some(String::new()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("series exists");
        assert_eq!(series.book_count, 1);
        assert_eq!(
            series.books[0].title.as_deref(),
            Some("Saga: Book One"),
            "the unaffected book stays; the cleared one drops out",
        );
    }

    #[tokio::test]
    async fn get_series_override_match_is_case_insensitive() {
        // The CTE's `series_name` column is BINARY by default — without
        // `COLLATE NOCASE` on the filter, an override that differs only
        // by case from the canonical series row fails to match, even
        // though `series.name` is `UNIQUE COLLATE NOCASE`.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let saga_id = series_id_by_name(&pool, "Saga").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        // Override uses lowercase casing; canonical row is "Saga".
        let ov = MetadataOverrides {
            series: Some("saga".into()),
            series_index: Some("3".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("series exists");
        let titles: Vec<_> = series
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert!(
            titles.contains(&"Standalone".to_string()),
            "lowercase override should still match NOCASE series row, got {titles:?}",
        );
    }

    #[tokio::test]
    async fn get_author_empty_string_series_index_sorts_last() {
        // Mirror of get_series: clearing the position field (`Some("")`)
        // used to CAST('') to 0.0 in get_author's ORDER BY and sort the
        // book to the front of the author's shelf. NULLIF drops it to NULL
        // so NULLS LAST trails it behind positioned books.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let author_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let book_one = books.iter().find(|b| b.filename == "saga1.epub").unwrap();
        let uuid = book_one.unique_identifier.clone().unwrap();

        // Keep Book One (canonical Saga #1) but clear its position.
        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some(String::new()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let author = get_author(&pool, author_id)
            .await
            .unwrap()
            .expect("author exists");
        let titles: Vec<_> = author
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        let pos = |t: &str| titles.iter().position(|x| x == t).unwrap();
        assert!(
            pos("Saga: Book Two") < pos("Saga: Book One"),
            "cleared series_index should trail the positioned book, got {titles:?}",
        );
        assert_ne!(
            titles.first().map(String::as_str),
            Some("Saga: Book One"),
            "cleared series_index must not sort to the front, got {titles:?}",
        );
    }

    #[tokio::test]
    async fn get_series_empty_string_series_index_sorts_last() {
        // `Some("")` from the edit form (user cleared the position
        // field) was sorting to the front because `CAST('' AS REAL)`
        // returns 0.0. NULLIF on the override value drops it to NULL,
        // and ORDER BY ... NULLS LAST trails it after positioned books.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let saga_id = series_id_by_name(&pool, "Saga").await;

        let books = list_books(&pool, "/lib").await.unwrap();
        let standalone = books
            .iter()
            .find(|b| b.filename == "standalone.epub")
            .unwrap();
        let uuid = standalone.unique_identifier.clone().unwrap();

        // Add Standalone to Saga but clear its position.
        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some(String::new()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let series = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("series exists");
        let titles: Vec<_> = series
            .books
            .iter()
            .map(|b| b.title.clone().unwrap_or_default())
            .collect();
        assert_eq!(
            titles,
            vec![
                "Saga: Book One".to_string(),
                "Saga: Book Two".to_string(),
                "Standalone".to_string(),
            ],
            "empty-string series_index should trail positioned books, not lead them",
        );
    }

    #[tokio::test]
    async fn get_series_pins_series_id_for_books_moved_between_series() {
        // A book canonically in Series A overridden into Series B used
        // to come back from get_series(B) with `series_id = Some(A)`
        // (BOOK_COLUMNS reads only books_series_link), so the card on
        // B's page would link back to /series/A. The fix pins
        // series_id/series unconditionally to the requested parent.
        let (pool, _guard) = seed_discovery_fixture().await;
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;
        let pioneers_id = series_id_by_name(&pool, "Pioneers").await;

        // "Other Story" is canonically in Pioneers; override moves it
        // into Saga. Verify that opening Saga's page returns the book
        // pinned to Saga's id, not Pioneers'.
        let books = list_books(&pool, "/lib").await.unwrap();
        let other = books.iter().find(|b| b.filename == "other.epub").unwrap();
        let uuid = other.unique_identifier.clone().unwrap();

        let saga_id = series_id_by_name(&pool, "Saga").await;
        let ov = MetadataOverrides {
            series: Some("Saga".into()),
            series_index: Some("5".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let saga = get_series(&pool, saga_id)
            .await
            .unwrap()
            .expect("Saga exists");
        let moved = saga
            .books
            .iter()
            .find(|b| b.title.as_deref() == Some("Other Story"))
            .expect("override moved Other Story into Saga");
        assert_eq!(
            moved.series_id,
            Some(saga_id),
            "card on Saga's page must link back to Saga, not the canonical Pioneers",
        );
        assert_eq!(moved.series.as_deref(), Some("Saga"));

        // And it should be gone from Pioneers' page.
        let pioneers = get_series(&pool, pioneers_id)
            .await
            .unwrap()
            .expect("Pioneers exists");
        assert!(
            !pioneers
                .books
                .iter()
                .any(|b| b.title.as_deref() == Some("Other Story")),
            "override moved Other Story off Pioneers",
        );
    }

    #[tokio::test]
    async fn get_book_leaves_series_id_none_when_override_series_unknown() {
        // If the override sets a series name that no other book uses, the
        // series table won't have a row to point at — backfill must
        // leave series_id None rather than fabricating one.
        let _covers = CoversTempDir::new("override_series_unknown");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "alone.epub",
                Some("Alone"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let book = &books[0];
        let uuid = book.unique_identifier.clone().unwrap();
        let id = book.id;

        let ov = MetadataOverrides {
            series: Some("A Series That Does Not Yet Exist".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(
            merged.series.as_deref(),
            Some("A Series That Does Not Yet Exist")
        );
        assert_eq!(merged.series_id, None);
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
    async fn list_books_merges_overrides_in_bulk() {
        let _covers = CoversTempDir::new("bulk_merge");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("Book A"), &["Author A"], &[], None, None),
                indexed("b.epub", Some("Book B"), &["Author B"], &[], None, None),
                indexed("c.epub", Some("Book C"), &["Author C"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid_a = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Book A"))
            .unwrap()
            .unique_identifier
            .clone()
            .unwrap();
        let uuid_c = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Book C"))
            .unwrap()
            .unique_identifier
            .clone()
            .unwrap();

        // Override A and C only.
        upsert_metadata_overrides(
            &pool,
            &uuid_a,
            &MetadataOverrides {
                title: Some("Edited A".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();
        upsert_metadata_overrides(
            &pool,
            &uuid_c,
            &MetadataOverrides {
                title: Some("Edited C".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books
            .iter()
            .find(|b| b.unique_identifier.as_deref() == Some(&uuid_a))
            .unwrap();
        let b = books
            .iter()
            .find(|b| b.title.as_deref() == Some("Book B"))
            .unwrap();
        let c = books
            .iter()
            .find(|b| b.unique_identifier.as_deref() == Some(&uuid_c))
            .unwrap();

        assert_eq!(a.title.as_deref(), Some("Edited A"));
        assert!(a.has_override);
        assert_eq!(b.title.as_deref(), Some("Book B"));
        assert!(!b.has_override);
        assert_eq!(c.title.as_deref(), Some("Edited C"));
        assert!(c.has_override);
    }

    #[tokio::test]
    async fn delete_overrides_reverts_to_scanned() {
        let _covers = CoversTempDir::new("revert");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "revert.epub",
                Some("Original"),
                &["Author"],
                &["fiction"],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        let id = books[0].id;

        // Override.
        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                title: Some("Changed".into()),
                subjects: Some(vec!["sci-fi".into()]),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();
        let merged = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(merged.title.as_deref(), Some("Changed"));

        // Delete overrides — should revert to scanned.
        delete_metadata_overrides(&pool, &uuid).await.unwrap();
        let reverted = get_book(&pool, id).await.unwrap().unwrap();
        assert_eq!(reverted.title.as_deref(), Some("Original"));
        assert_eq!(reverted.subjects, vec!["fiction"]);
        assert!(!reverted.has_override);
    }

    /// Verify that `MetadataOverrides::merge` correctly layers a second edit
    /// on top of a first without losing the first edit's fields.
    #[tokio::test]
    async fn merge_preserves_prior_overrides() {
        let first = MetadataOverrides {
            title: Some("Edited Title".into()),
            publisher: Some("Edited Publisher".into()),
            ..Default::default()
        };
        let second = MetadataOverrides {
            description: Some("New description".into()),
            ..Default::default()
        };
        let merged = first.merge(&second);
        // second's description wins
        assert_eq!(merged.description.as_deref(), Some("New description"));
        // first's title and publisher are preserved (not wiped by None)
        assert_eq!(merged.title.as_deref(), Some("Edited Title"));
        assert_eq!(merged.publisher.as_deref(), Some("Edited Publisher"));
        // unset in both stays None
        assert_eq!(merged.language, None);
    }

    // ── search_palette ──────────────────────────────────────────────

    #[tokio::test]
    async fn palette_books_match_title() {
        let _covers = CoversTempDir::new("palette_books");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dracula"),
                    &["Bram Stoker"],
                    &["Horror"],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Frankenstein"),
                    &["Mary Shelley"],
                    &["Horror"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "dracula").await.unwrap();
        assert_eq!(results.books.len(), 1);
        assert_eq!(results.books[0].title, "Dracula");
        assert_eq!(results.books[0].author_display, "Bram Stoker");
        assert!(results.books[0].formats.contains(&"EPUB".to_string()));
    }

    #[tokio::test]
    async fn palette_authors_match_substring() {
        let _covers = CoversTempDir::new("palette_authors");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Babel"),
                &["R. F. Kuang"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "kuang").await.unwrap();
        assert!(!results.authors.is_empty(), "should match author substring");
        assert_eq!(results.authors[0].name, "R. F. Kuang");
        assert_eq!(results.authors[0].book_count, 1);
    }

    #[tokio::test]
    async fn palette_series_match() {
        let _covers = CoversTempDir::new("palette_series");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Book One"),
                    &["Author"],
                    &[],
                    Some(("Poppy War", "1")),
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Book Two"),
                    &["Author"],
                    &[],
                    Some(("Poppy War", "2")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "poppy").await.unwrap();
        assert!(!results.series.is_empty(), "should match series substring");
        assert_eq!(results.series[0].name, "Poppy War");
        assert_eq!(results.series[0].book_count, 2);
    }

    #[tokio::test]
    async fn palette_tags_match() {
        let _covers = CoversTempDir::new("palette_tags");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Author"],
                &["Dark academia"],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "academia").await.unwrap();
        assert!(!results.tags.is_empty(), "should match tag substring");
        assert_eq!(results.tags[0].name, "Dark academia");
        assert_eq!(results.tags[0].book_count, 1);
    }

    /// Bug #1 (display side): the palette must show the overridden title,
    /// not the canonical scanned `b.title`, so what the user clicks matches
    /// what they searched for.
    #[tokio::test]
    async fn palette_book_hit_uses_overridden_title() {
        let _covers = CoversTempDir::new("palette_override_title");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Scanned Title"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                title: Some("Edited Title".into()),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let palette = search_palette(&pool, "/lib", "Edited").await.unwrap();
        assert_eq!(palette.books.len(), 1);
        assert_eq!(palette.books[0].title, "Edited Title");
    }

    /// Bug #1 (display side): overriding the creators list rebuilds the
    /// comma-joined `author_display` so the palette subtitle matches the
    /// detail page.
    #[tokio::test]
    async fn palette_book_hit_uses_overridden_author_display() {
        let _covers = CoversTempDir::new("palette_override_authors");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Searchable"),
                &["Original Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let uuid = list_books(&pool, "/lib").await.unwrap()[0]
            .unique_identifier
            .clone()
            .unwrap();
        upsert_metadata_overrides(
            &pool,
            &uuid,
            &MetadataOverrides {
                creators: Some(vec![
                    Contributor {
                        name: "First Override".into(),
                        ..Default::default()
                    },
                    Contributor {
                        name: "Second Override".into(),
                        ..Default::default()
                    },
                ]),
                ..Default::default()
            },
            false,
            user_id,
        )
        .await
        .unwrap();

        let palette = search_palette(&pool, "/lib", "Searchable").await.unwrap();
        assert_eq!(palette.books.len(), 1);
        assert_eq!(
            palette.books[0].author_display,
            "First Override, Second Override"
        );
    }

    /// Palette book hits should surface a user-uploaded cover even when the
    /// scanned book had no cover. Mirrors `apply_overrides` so the palette
    /// row doesn't go cover-less for an override-only cover.
    #[tokio::test]
    async fn palette_book_hit_uses_overridden_cover() {
        let _covers = CoversTempDir::new("palette_override_cover");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Indexed book with no scanned cover.
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "p.epub",
                Some("Coverless Searchable"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        let book = list_books(&pool, "/lib").await.unwrap().remove(0);
        let uuid = book.unique_identifier.clone().unwrap();

        // Set has_cover_override = true with no text edits.
        upsert_metadata_overrides(&pool, &uuid, &MetadataOverrides::default(), true, user_id)
            .await
            .unwrap();

        let palette = search_palette(&pool, "/lib", "Coverless").await.unwrap();
        assert_eq!(palette.books.len(), 1);
        assert_eq!(
            palette.books[0].cover_url,
            Some(format!("/api/covers/{uuid}"))
        );
    }

    // #128: lock the wiring between the palette and `build_fts_match`'s
    // facet prefixes. A regression in the facet parser could otherwise
    // silently break palette tag:/author:/series: queries without any
    // palette test failing.

    #[tokio::test]
    async fn palette_book_matches_tag_facet() {
        let _covers = CoversTempDir::new("palette_tag_facet");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dracula"),
                    &["Bram Stoker"],
                    &["vampires"],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Frankenstein"),
                    &["Mary Shelley"],
                    &["monsters"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "tag:vampires").await.unwrap();
        let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
        assert!(
            titles.contains(&"Dracula"),
            "tag:vampires should match Dracula, got {titles:?}"
        );
        assert!(
            !titles.contains(&"Frankenstein"),
            "tag:vampires should not match Frankenstein"
        );
    }

    #[tokio::test]
    async fn palette_book_matches_author_facet() {
        let _covers = CoversTempDir::new("palette_author_facet");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Dracula"),
                    &["Bram Stoker"],
                    &["horror"],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Frankenstein"),
                    &["Mary Shelley"],
                    &["horror"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "author:stoker")
            .await
            .unwrap();
        let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
        assert!(
            titles.contains(&"Dracula"),
            "author:stoker should match Dracula, got {titles:?}"
        );
        assert!(
            !titles.contains(&"Frankenstein"),
            "author:stoker should not match Frankenstein"
        );
    }

    #[tokio::test]
    async fn palette_book_matches_series_facet() {
        let _covers = CoversTempDir::new("palette_series_facet");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Book One"),
                    &["Author A"],
                    &[],
                    Some(("Dracula Chronicles", "1")),
                    None,
                ),
                indexed("b.epub", Some("Unrelated"), &["Author B"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "series:dracula")
            .await
            .unwrap();
        let titles: Vec<&str> = results.books.iter().map(|b| b.title.as_str()).collect();
        assert!(
            titles.contains(&"Book One"),
            "series:dracula should match Book One, got {titles:?}"
        );
        assert!(
            !titles.contains(&"Unrelated"),
            "series:dracula should not match Unrelated"
        );
    }

    #[tokio::test]
    async fn palette_empty_query_returns_empty() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let results = search_palette(&pool, "/lib", "   ").await.unwrap();
        assert!(results.books.is_empty());
        assert!(results.authors.is_empty());
        assert!(results.series.is_empty());
        assert!(results.tags.is_empty());
        assert_eq!(results.query, "");
    }

    #[tokio::test]
    async fn palette_no_results() {
        let _covers = CoversTempDir::new("palette_no_results");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("A"), &["Author"], &[], None, None)],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "zzzznonexistent")
            .await
            .unwrap();
        assert!(results.books.is_empty());
        assert!(results.authors.is_empty());
        assert!(results.series.is_empty());
        assert!(results.tags.is_empty());
    }

    #[tokio::test]
    async fn palette_scoped_to_library() {
        let _covers = CoversTempDir::new("palette_scoped");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib-a",
            vec![indexed(
                "a.epub",
                Some("Alpha"),
                &["Tolkien"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![indexed(
                "b.epub",
                Some("Beta"),
                &["Tolkien"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib-a", "tolkien").await.unwrap();
        // Books should only include lib-a
        assert_eq!(results.books.len(), 1);
        assert_eq!(results.books[0].title, "Alpha");
        // Author book_count should be 1 (scoped to lib-a), not 2
        assert_eq!(results.authors[0].book_count, 1);
    }

    #[tokio::test]
    async fn palette_duration_populated() {
        let _covers = CoversTempDir::new("palette_duration");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("A"), &["Author"], &[], None, None)],
        )
        .await
        .unwrap();

        let results = search_palette(&pool, "/lib", "author").await.unwrap();
        // duration_ms should be populated (at least 0 — we just check it's set)
        assert!(results.duration_ms < 10000, "duration should be reasonable");
    }

    /// #127 regression coverage: after collapsing the correlated
    /// `book_count` / `EXISTS` subqueries into a single JOIN+GROUP BY,
    /// `l.path = ?1` must still be applied **before** the aggregate so the
    /// scoped library's count doesn't pick up rows from sibling libraries.
    /// This exercises all three taxonomies (authors, series, tags) plus
    /// ordering — the seeded set has 3 matching books in /lib-a and 2 in
    /// /lib-b for the same author/series/tag, and the rare "Sole" author
    /// only appears in /lib-b, so it must be absent from /lib-a results.
    #[tokio::test]
    async fn palette_taxonomy_counts_scoped_per_library() {
        let _covers = CoversTempDir::new("palette_taxonomy_scoped");
        let pool = init_db("sqlite::memory:").await.unwrap();

        replace_books(
            &pool,
            "/lib-a",
            vec![
                indexed(
                    "a1.epub",
                    Some("Alpha One"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "1")),
                    None,
                ),
                indexed(
                    "a2.epub",
                    Some("Alpha Two"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "2")),
                    None,
                ),
                indexed(
                    "a3.epub",
                    Some("Alpha Three"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "3")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![
                indexed(
                    "b1.epub",
                    Some("Beta One"),
                    &["Shared Author", "Sole Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "1")),
                    None,
                ),
                indexed(
                    "b2.epub",
                    Some("Beta Two"),
                    &["Shared Author"],
                    &["Shared Tag"],
                    Some(("Shared Series", "2")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        // Scope to /lib-a — author/series/tag counts must be 3, not 5.
        let results = search_palette(&pool, "/lib-a", "Shared").await.unwrap();

        let author = results
            .authors
            .iter()
            .find(|a| a.name == "Shared Author")
            .expect("Shared Author present in /lib-a results");
        assert_eq!(
            author.book_count, 3,
            "author count must be scoped to /lib-a, got {results:?}"
        );
        assert!(
            !results.authors.iter().any(|a| a.name == "Sole Author"),
            "Sole Author lives only in /lib-b and must not appear"
        );

        let series = results
            .series
            .iter()
            .find(|s| s.name == "Shared Series")
            .expect("Shared Series present in /lib-a results");
        assert_eq!(
            series.book_count, 3,
            "series count must be scoped to /lib-a"
        );

        let tag = results
            .tags
            .iter()
            .find(|t| t.name == "Shared Tag")
            .expect("Shared Tag present in /lib-a results");
        assert_eq!(tag.book_count, 3, "tag count must be scoped to /lib-a");

        // Cross-check /lib-b counts to make sure the same query returns 2.
        let results_b = search_palette(&pool, "/lib-b", "Shared").await.unwrap();
        let author_b = results_b
            .authors
            .iter()
            .find(|a| a.name == "Shared Author")
            .expect("Shared Author present in /lib-b results");
        assert_eq!(author_b.book_count, 2);
        let series_b = results_b
            .series
            .iter()
            .find(|s| s.name == "Shared Series")
            .expect("Shared Series present in /lib-b results");
        assert_eq!(series_b.book_count, 2);
        let tag_b = results_b
            .tags
            .iter()
            .find(|t| t.name == "Shared Tag")
            .expect("Shared Tag present in /lib-b results");
        assert_eq!(tag_b.book_count, 2);
    }

    #[tokio::test]
    async fn palette_author_count_reflects_overrides() {
        // F5.1: the palette author count must match the merged
        // (override-aware) view, not the raw `books_authors_link` count.
        // Repro of the "Sanderson, Brandon still says 4 books" report:
        // every canonical book for an author was reassigned to a
        // differently-named author through the metadata edit form, so the
        // palette must report 0 books for the source name and the full
        // count for the destination name.
        let _covers = CoversTempDir::new("palette_author_count_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        // Two books canonically by "Last, First", plus one book by the
        // already-correct "First Last" so the destination author has a
        // canonical anchor (palette visibility requires ≥1 canonical link).
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Last, First"], &[], None, None),
                indexed("b.epub", Some("B"), &["Last, First"], &[], None, None),
                indexed("c.epub", Some("C"), &["First Last"], &[], None, None),
            ],
        )
        .await
        .unwrap();

        // User edits a.epub and b.epub through the metadata form to
        // rename their author to "First Last" — overrides only, no
        // change to the relational link table.
        let books = list_books(&pool, "/lib").await.unwrap();
        for filename in ["a.epub", "b.epub"] {
            let book = books.iter().find(|b| b.filename == filename).unwrap();
            let uuid = book.unique_identifier.clone().unwrap();
            let ov = MetadataOverrides {
                creators: Some(vec![Contributor {
                    name: "First Last".into(),
                    role: Some("aut".into()),
                    file_as: None,
                    id: None,
                }]),
                ..Default::default()
            };
            upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
                .await
                .unwrap();
        }

        let results = search_palette(&pool, "/lib", "Last").await.unwrap();

        // Source author still visible (canonical anchor remains), but
        // count must reflect the effective view: 0 books.
        let source = results
            .authors
            .iter()
            .find(|a| a.name == "Last, First")
            .expect("source author still appears in palette");
        assert_eq!(
            source.book_count, 0,
            "renamed-away author must report effective count 0, got {results:?}",
        );

        // Destination author picks up the override-renamed books on top
        // of its own canonical anchor: 1 + 2 = 3.
        let dest = results
            .authors
            .iter()
            .find(|a| a.name == "First Last")
            .expect("destination author present");
        assert_eq!(
            dest.book_count, 3,
            "destination author must include override-renamed books, got {results:?}",
        );
    }

    #[tokio::test]
    async fn palette_tag_count_reflects_overrides() {
        // F5.1: same shape for tags. `overrides.subjects` replaces the
        // canonical tag list wholesale, so a book moved between tags
        // must shift both counts.
        let _covers = CoversTempDir::new("palette_tag_count_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["X"], &["tag-source"], None, None),
                indexed("b.epub", Some("B"), &["X"], &["tag-source"], None, None),
                indexed("c.epub", Some("C"), &["X"], &["tag-dest"], None, None),
            ],
        )
        .await
        .unwrap();

        // Move a.epub off tag-source and onto tag-dest via override.
        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books.iter().find(|b| b.filename == "a.epub").unwrap();
        let uuid = a.unique_identifier.clone().unwrap();
        let ov = MetadataOverrides {
            subjects: Some(vec!["tag-dest".into()]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let results = search_palette(&pool, "/lib", "tag-").await.unwrap();
        let source = results
            .tags
            .iter()
            .find(|t| t.name == "tag-source")
            .expect("tag-source still visible (canonical anchor remains)");
        assert_eq!(
            source.book_count, 1,
            "tag-source should drop a.epub after override, got {results:?}",
        );
        let dest = results
            .tags
            .iter()
            .find(|t| t.name == "tag-dest")
            .expect("tag-dest present");
        assert_eq!(
            dest.book_count, 2,
            "tag-dest should add the override-tagged a.epub, got {results:?}",
        );
    }

    #[tokio::test]
    async fn palette_series_count_reflects_overrides() {
        // F5.1: same shape as palette_author_count_reflects_overrides
        // but for the series tile. Books moved into a series via
        // `overrides.series` must add to the destination count; books
        // moved out drop from the source count.
        let _covers = CoversTempDir::new("palette_series_count_overrides");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("A"),
                    &["X"],
                    &[],
                    Some(("Series Source", "1")),
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("B"),
                    &["X"],
                    &[],
                    Some(("Series Source", "2")),
                    None,
                ),
                indexed(
                    "c.epub",
                    Some("C"),
                    &["X"],
                    &[],
                    Some(("Series Dest", "1")),
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        // Move a.epub from Series Source to Series Dest via override.
        let books = list_books(&pool, "/lib").await.unwrap();
        let a = books.iter().find(|b| b.filename == "a.epub").unwrap();
        let uuid = a.unique_identifier.clone().unwrap();
        let ov = MetadataOverrides {
            series: Some("Series Dest".into()),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        // "Series" matches both names.
        let results = search_palette(&pool, "/lib", "Series").await.unwrap();
        let source = results
            .series
            .iter()
            .find(|s| s.name == "Series Source")
            .expect("Series Source still visible (canonical anchor remains)");
        assert_eq!(
            source.book_count, 1,
            "Series Source should count only b.epub after a.epub is overridden away, got {results:?}",
        );
        let dest = results
            .series
            .iter()
            .find(|s| s.name == "Series Dest")
            .expect("Series Dest present");
        assert_eq!(
            dest.book_count, 2,
            "Series Dest should count its canonical c.epub plus the override-moved a.epub, got {results:?}",
        );
    }

    #[tokio::test]
    async fn palette_series_author_display_reflects_override() {
        // F5.1: the "by X" line on a series tile must follow the first
        // book's effective creator, not the canonical one — otherwise
        // renaming the author through the metadata edit form leaves the
        // palette showing the old name.
        let _covers = CoversTempDir::new("palette_series_author_display");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
            .await
            .unwrap()
            .id;

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "k1.epub",
                Some("K1"),
                &["Old Name"],
                &[],
                Some(("Kingsway", "1")),
                None,
            )],
        )
        .await
        .unwrap();

        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();

        let ov = MetadataOverrides {
            creators: Some(vec![Contributor {
                name: "New Name".into(),
                role: Some("aut".into()),
                file_as: None,
                id: None,
            }]),
            ..Default::default()
        };
        upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
            .await
            .unwrap();

        let results = search_palette(&pool, "/lib", "Kingsway").await.unwrap();
        let kingsway = results
            .series
            .iter()
            .find(|s| s.name == "Kingsway")
            .expect("Kingsway present");
        assert_eq!(
            kingsway.author_display.as_deref(),
            Some("New Name"),
            "palette author line must follow override.creators, got {results:?}",
        );
    }

    /// #127: capture `EXPLAIN QUERY PLAN` for each of the three rewritten
    /// taxonomy queries and assert the planner uses the link-table indexes.
    /// This is a structural check — it doesn't pin the literal plan string
    /// (SQLite's wording can shift across point releases) but it does fail
    /// loudly if any of the link tables fall back to a full SCAN, which
    /// would defeat the whole point of this optimization.
    #[tokio::test]
    async fn palette_taxonomy_query_plans_use_indexes() {
        let pool = init_db("sqlite::memory:").await.unwrap();

        async fn plan_text(pool: &SqlitePool, sql: &str) -> String {
            let rows = sqlx::query(&format!("EXPLAIN QUERY PLAN {sql}"))
                .bind("/lib")
                .bind("%x%")
                .bind(5_i32)
                .fetch_all(pool)
                .await
                .unwrap();
            rows.iter()
                .map(|r| r.get::<String, _>("detail"))
                .collect::<Vec<_>>()
                .join("\n")
        }

        // Authors — override-aware count, must still drive through the
        // covering `books_authors_link` index when checking visibility
        // and when the no-override branch of the CASE runs.
        let plan = plan_text(
            &pool,
            "SELECT a.id, a.name, \
              (SELECT COUNT(*) FROM books b \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                WHERE l2.path = ?1 \
                  AND CASE \
                        WHEN mo.book_uuid IS NOT NULL \
                             AND json_type(mo.overrides, '$.creators') IS NOT NULL \
                          THEN EXISTS (SELECT 1 FROM json_each(mo.overrides, '$.creators') je \
                                        WHERE json_extract(je.value, '$.name') = a.name) \
                        ELSE EXISTS (SELECT 1 FROM books_authors_link bal \
                                      WHERE bal.book = b.id AND bal.author = a.id) \
                      END \
              ) AS book_count \
             FROM authors a \
             WHERE a.name LIKE ?2 ESCAPE '\\' \
               AND EXISTS (SELECT 1 FROM books_authors_link bal \
                             JOIN books b ON b.id = bal.book \
                             JOIN libraries l ON l.id = b.library_id \
                            WHERE bal.author = a.id AND l.path = ?1) \
             ORDER BY book_count DESC, a.name \
             LIMIT ?3",
        )
        .await;
        assert!(
            !plan.contains("SCAN books_authors_link") && !plan.contains("SCAN bal"),
            "authors plan should not full-scan the link table:\n{plan}"
        );

        // Series — override-aware count + author_display, must still drive
        // through the `books_series_link` index for visibility and the
        // no-override branch of the CASE.
        let plan = plan_text(
            &pool,
            "SELECT s.id, s.name, \
              (SELECT COUNT(*) FROM books b \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                WHERE l2.path = ?1 \
                  AND CASE \
                        WHEN mo.book_uuid IS NOT NULL \
                             AND json_type(mo.overrides, '$.series') IS NOT NULL \
                          THEN json_extract(mo.overrides, '$.series') = s.name \
                        ELSE EXISTS (SELECT 1 FROM books_series_link bsl \
                                      WHERE bsl.book = b.id AND bsl.series = s.id) \
                      END \
              ) AS book_count \
             FROM series s \
             WHERE s.name LIKE ?2 ESCAPE '\\' \
               AND EXISTS (SELECT 1 FROM books_series_link bsl \
                             JOIN books b ON b.id = bsl.book \
                             JOIN libraries l ON l.id = b.library_id \
                            WHERE bsl.series = s.id AND l.path = ?1) \
             ORDER BY book_count DESC, s.name \
             LIMIT ?3",
        )
        .await;
        assert!(
            !plan.contains("SCAN books_series_link") && !plan.contains("SCAN bsl"),
            "series plan should not full-scan the link table:\n{plan}"
        );

        // Tags — override-aware count, must still drive through the
        // `books_tags_link` index for visibility and the no-override
        // branch of the CASE.
        let plan = plan_text(
            &pool,
            "SELECT t.id, t.name, \
              (SELECT COUNT(*) FROM books b \
                 JOIN libraries l2 ON l2.id = b.library_id \
                 LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid \
                WHERE l2.path = ?1 \
                  AND CASE \
                        WHEN mo.book_uuid IS NOT NULL \
                             AND json_type(mo.overrides, '$.subjects') IS NOT NULL \
                          THEN EXISTS (SELECT 1 FROM json_each(mo.overrides, '$.subjects') je \
                                        WHERE je.value = t.name) \
                        ELSE EXISTS (SELECT 1 FROM books_tags_link btl \
                                      WHERE btl.book = b.id AND btl.tag = t.id) \
                      END \
              ) AS book_count \
             FROM tags t \
             WHERE t.name LIKE ?2 ESCAPE '\\' \
               AND EXISTS (SELECT 1 FROM books_tags_link btl \
                             JOIN books b ON b.id = btl.book \
                             JOIN libraries l ON l.id = b.library_id \
                            WHERE btl.tag = t.id AND l.path = ?1) \
             ORDER BY book_count DESC, t.name \
             LIMIT ?3",
        )
        .await;
        assert!(
            !plan.contains("SCAN books_tags_link") && !plan.contains("SCAN btl"),
            "tags plan should not full-scan the link table:\n{plan}"
        );
    }

    // Additional coverage for core book query functions.

    #[tokio::test]
    async fn list_books_filters_by_library_path() {
        let _covers = CoversTempDir::new("list_books_filter_lib");
        let pool = init_db("sqlite::memory:").await.unwrap();

        replace_books(
            &pool,
            "/lib-a",
            vec![
                indexed("a1.epub", Some("Alpha One"), &["Author A"], &[], None, None),
                indexed("a2.epub", Some("Alpha Two"), &["Author A"], &[], None, None),
            ],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![indexed(
                "b1.epub",
                Some("Beta One"),
                &["Author B"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let lib_a = list_books(&pool, "/lib-a").await.unwrap();
        let lib_b = list_books(&pool, "/lib-b").await.unwrap();

        assert_eq!(lib_a.len(), 2, "lib-a should return only its two books");
        let mut titles_a: Vec<String> = lib_a.iter().filter_map(|b| b.title.clone()).collect();
        titles_a.sort();
        assert_eq!(titles_a, vec!["Alpha One", "Alpha Two"]);

        assert_eq!(lib_b.len(), 1, "lib-b should return only its one book");
        assert_eq!(lib_b[0].title.as_deref(), Some("Beta One"));
    }

    #[tokio::test]
    async fn list_books_returns_empty_for_unknown_path() {
        let _covers = CoversTempDir::new("list_books_unknown");
        let pool = init_db("sqlite::memory:").await.unwrap();

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Title"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let hits = list_books(&pool, "/does-not-exist").await.unwrap();
        assert!(
            hits.is_empty(),
            "unknown library path should yield an empty vec (no error)"
        );
    }

    #[tokio::test]
    async fn list_books_returns_empty_for_empty_db() {
        let _covers = CoversTempDir::new("list_books_empty_db");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let hits = list_books(&pool, "/lib").await.unwrap();
        assert!(hits.is_empty(), "empty DB should yield an empty vec");
    }

    #[tokio::test]
    async fn search_books_handles_bare_asterisk_without_error() {
        // A raw `*` is an FTS5 operator; the sanitizer must quote it so MATCH
        // doesn't reject the expression as a syntax error. We assert the call
        // succeeds — not a particular hit shape — because the goal is "no panic
        // / no sqlx parse error".
        let _covers = CoversTempDir::new("fts_asterisk");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Anything"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "*")
            .await
            .expect("sanitizer should guard MATCH against the bare `*` operator");
        // `*` alone has no literal token to match; an empty result is fine.
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_books_handles_bare_double_quote_without_error() {
        // A raw `"` is the FTS5 phrase delimiter. Without sanitization, MATCH
        // would reject this with a parse error and the call would `Err`.
        let _covers = CoversTempDir::new("fts_dquote");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Anything"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "\"")
            .await
            .expect("sanitizer should guard MATCH against a bare `\"` operator");
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_books_returns_empty_for_unknown_library() {
        // Even with a real match in another library, the WHERE l.path = ?
        // clause must scope results to the requested library.
        let _covers = CoversTempDir::new("search_books_unknown_lib");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib-a",
            vec![indexed(
                "a.epub",
                Some("Findable"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib-b", "Findable").await.unwrap();
        assert!(
            hits.is_empty(),
            "query against a non-existent library must not leak rows from another library"
        );
    }

    // -------------------------------------------------------------------------
    // F1.11 author profile photo tests
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn author_photo_roundtrips_manual_upload() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let bytes = b"\xFF\xD8\xFFfake-jpeg".to_vec();
        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(&bytes),
        )
        .await
        .unwrap();

        let (mime, fetched) = get_author_photo(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(mime, "image/jpeg");
        assert_eq!(fetched, bytes);
    }

    #[tokio::test]
    async fn author_photo_letter_marker_returns_none() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();

        assert!(get_author_photo(&pool, ada_id).await.unwrap().is_none());

        let (src, _) = author_photo_status(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::Letter);
    }

    #[tokio::test]
    async fn author_photo_status_none_when_unset() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;
        assert!(author_photo_status(&pool, ada_id).await.unwrap().is_none());
        assert!(get_author_photo(&pool, ada_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn author_photo_upsert_replaces_existing_row() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        // Letter marker first, then a manual upload replaces it.
        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();
        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/png"),
            Some(b"\x89PNG\r\n\x1a\nfake"),
        )
        .await
        .unwrap();

        let (src, _) = author_photo_status(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(src, AuthorPhotoSource::Manual);
        let (mime, _) = get_author_photo(&pool, ada_id).await.unwrap().unwrap();
        assert_eq!(mime, "image/png");
    }

    #[tokio::test]
    async fn author_photo_delete_clears_row() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(b"\xFF\xD8\xFFfoo"),
        )
        .await
        .unwrap();
        delete_author_photo(&pool, ada_id).await.unwrap();

        assert!(author_photo_status(&pool, ada_id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn get_author_populates_has_photo() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        // No row → false.
        let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
        assert!(!ada.has_photo, "no row should yield has_photo = false");

        // Letter marker → still false (negative-cache shouldn't render an img).
        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();
        let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
        assert!(
            !ada.has_photo,
            "letter marker should yield has_photo = false"
        );

        // Manual upload → true.
        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(b"\xFF\xD8\xFFfake"),
        )
        .await
        .unwrap();
        let ada = get_author(&pool, ada_id).await.unwrap().unwrap();
        assert!(ada.has_photo, "manual upload should yield has_photo = true");
    }

    /// `list_authors` mirrors the `get_author` has_photo semantics so the
    /// /authors index can pick the right avatar without a per-card detail
    /// fetch. Same three-state matrix: no row → false, `letter` marker →
    /// false, `manual` upload → true.
    #[tokio::test]
    async fn list_authors_populates_has_photo() {
        let (pool, _guard) = seed_discovery_fixture().await;
        let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

        let initial = list_authors(&pool, "/lib").await.unwrap();
        let ada = initial.iter().find(|a| a.id == ada_id).unwrap();
        assert!(!ada.has_photo, "no row should yield has_photo = false");

        upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
            .await
            .unwrap();
        let after_letter = list_authors(&pool, "/lib").await.unwrap();
        let ada = after_letter.iter().find(|a| a.id == ada_id).unwrap();
        assert!(
            !ada.has_photo,
            "letter marker should yield has_photo = false"
        );

        upsert_author_photo(
            &pool,
            ada_id,
            AuthorPhotoSource::Manual,
            None,
            Some("image/jpeg"),
            Some(b"\xFF\xD8\xFFfake"),
        )
        .await
        .unwrap();
        let after_upload = list_authors(&pool, "/lib").await.unwrap();
        let ada = after_upload.iter().find(|a| a.id == ada_id).unwrap();
        assert!(ada.has_photo, "manual upload should yield has_photo = true");
    }

    // -------------------------------------------------------------------------
    // ignored_authors blocklist (F5.9-lite reindex-resurrection guard)
    // -------------------------------------------------------------------------

    // -------------------------------------------------------------------------
    // delete_author (F5.9-lite admin "Delete author" primitive)
    // -------------------------------------------------------------------------

    #[tokio::test]
    async fn delete_author_removes_links_and_inserts_blocklist_row() {
        let _covers = CoversTempDir::new("delete_author_basic");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Junk Author"], &[], None, None),
                indexed(
                    "b.epub",
                    Some("B"),
                    &["Junk Author", "Real Author"],
                    &[],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let junk_id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
            .bind("Junk Author")
            .fetch_one(&pool)
            .await
            .unwrap();

        let unlinked = delete_author(&pool, junk_id).await.unwrap();
        assert_eq!(unlinked, 2, "both books should report as un-linked");

        let junk_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE id = ?")
            .bind(junk_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(junk_count, 0, "authors row should be gone");

        let link_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM books_authors_link WHERE author = ?")
                .bind(junk_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            link_count, 0,
            "no link rows should remain for deleted author"
        );

        let blocklist_count: i64 =
            sqlx::query_scalar("SELECT COUNT(*) FROM ignored_authors WHERE name = ?")
                .bind("Junk Author")
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(blocklist_count, 1, "name must be added to ignored_authors");

        // Real Author on book B should still be linked.
        let books = list_books(&pool, "/lib").await.unwrap();
        let b = books
            .iter()
            .find(|x| x.title.as_deref() == Some("B"))
            .unwrap();
        let creators: Vec<&str> = b.creators.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(creators, vec!["Real Author"]);
    }

    #[tokio::test]
    async fn delete_author_is_no_op_for_missing_id() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let unlinked = delete_author(&pool, 99_999).await.unwrap();
        assert_eq!(unlinked, 0);
        let blocklist_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ignored_authors")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            blocklist_count, 0,
            "missing id must not leak a ghost blocklist row"
        );
    }

    #[tokio::test]
    async fn delete_author_survives_reindex() {
        // Full durability check: delete the junk author, then run the
        // reindex pipeline against a fixture that *still* lists the junk
        // contributor in its OPF (simulated via replace_books with the
        // same input). The blocklist row inserted by delete_author must
        // keep resolve_or_insert_author from re-creating the author.
        let _covers = CoversTempDir::new("delete_author_reindex");
        let pool = init_db("sqlite::memory:").await.unwrap();

        let junk_name = "calibre (8.0.0) [https://calibre-ebook.com]";
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Real Author", junk_name],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let junk_id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
            .bind(junk_name)
            .fetch_one(&pool)
            .await
            .unwrap();
        delete_author(&pool, junk_id).await.unwrap();

        // Simulated reindex: same OPF contents, run through the
        // indexing pipeline again. Without the blocklist guard this
        // would re-create the junk author and relink it.
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Real Author", junk_name],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        let junk_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE name = ?")
            .bind(junk_name)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            junk_after, 0,
            "second reindex must not resurrect a deleted author"
        );

        let books = list_books(&pool, "/lib").await.unwrap();
        let creators: Vec<&str> = books[0].creators.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(creators, vec!["Real Author"]);
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
}
