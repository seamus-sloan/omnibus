//! Transactional orchestrator for the indexer write path. Owns
//! `sync_books`, the per-bucket helpers (`sync_removed` / `sync_changed`
//! / `sync_new`), `replace_books`, and post-commit cover materialization.
//! All `books_fts` maintenance is delegated to the [`super::fts`]
//! choke-point (`upsert_fts` / `delete_fts`) rather than written inline.

use sqlx::{SqlitePool, Transaction};

use crate::covers::delete_cover_files_for;
use crate::settings::upsert_library;

use super::attach;

mod backfill;
mod changed;
mod new;
mod removed;
mod shared;

pub(super) use shared::{clear_missing_files_flag, materialize_new_covers};

use backfill::stamp_last_indexed;
use changed::sync_changed;
use new::sync_new;
use removed::sync_removed;

/// Crate-internal error wrapping `sqlx::Error` so `?` propagates cleanly in the audiobook sync helpers.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SyncError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<crate::settings::SettingsError> for SyncError {
    fn from(e: crate::settings::SettingsError) -> Self {
        match e {
            crate::settings::SettingsError::Db(inner) => SyncError::Db(inner),
            // `Validation` is only produced by `set_hardcover_api_key`, which
            // no sync path calls — keep the arm exhaustive but do not widen
            // `SyncError`'s surface for a case the caller graph can't reach.
            crate::settings::SettingsError::Validation(msg) => SyncError::Db(
                sqlx::Error::Protocol(format!("unexpected settings validation error: {msg}")),
            ),
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
/// 1. Upsert the `scan_roots` row.
/// 2. Mark Removed files missing (F2): drop each removed book's
///    `book_files` row but retain the `books` row, its links, FTS, and
///    soft-ref user data so the book stays in browse/search (the grid
///    hides it via `EXISTS book_files`) and the uuid survives.
/// 3. Update Changed in place (preserves `books.id`); wipe-and-rewrite
///    link rows + FTS row for each.
/// 4. Insert New (autoincrement assigns a fresh id).
/// 5. Backfill: UPDATE `book_files.(mtime_epoch, size_bytes)` only — no
///    OPF re-parse, no link writes, no FTS write. See the Backfill rule
///    in the [`crate::indexer`] module doc.
/// 6. Stamp `scan_roots.last_indexed`.
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
) -> anyhow::Result<()> {
    sync_books_with_progress(pool, library_path, plan, |_, _| {}).await
}

/// [`sync_books`] variant that calls `on_progress(processed, total)`
/// after each per-book write so the worker can surface a determinate
/// progress bar. `total` is the count of buckets that loop per book —
/// Changed + New. Removed and Backfill are batched and not reported as
/// per-book progress (they're invisible to the user-facing "Scanning"
/// step).
pub async fn sync_books_with_progress(
    pool: &SqlitePool,
    library_path: &str,
    plan: SyncPlan,
    mut on_progress: impl FnMut(u32, u32),
) -> anyhow::Result<()> {
    let total: u32 = (plan.changed_books.len() + plan.new_books.len())
        .try_into()
        .unwrap_or(u32::MAX);
    // Emit an initial (0, total) tick so the UI flips from indeterminate
    // spinner to determinate bar before the first per-book write lands.
    on_progress(0, total);

    let mut tx = pool.begin().await?;
    let library_id = upsert_library(&mut tx, library_path).await?;

    sync_removed(&mut tx, library_id, &plan.removed_uuids).await?;
    // Removed uuids that were cross-format attachments have no `books`
    // row — drop their `book_files` row + `merged_uuids` entry instead
    // (the target book survives, possibly fileless).
    attach::remove_attached_files(&mut tx, &plan.removed_uuids).await?;
    let mut processed: u32 = 0;
    let changed_covers = sync_changed(
        &mut tx,
        library_id,
        library_path,
        &plan.changed_books,
        || {
            processed = processed.saturating_add(1);
            on_progress(processed, total);
        },
    )
    .await?;
    let new_covers = sync_new(&mut tx, library_id, library_path, &plan.new_books, || {
        processed = processed.saturating_add(1);
        on_progress(processed, total);
    })
    .await?;
    super::backfill::backfill_stat_chunks(&mut tx, library_id, &plan.backfill).await?;
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
) -> anyhow::Result<()> {
    let removed_uuids: Vec<String> = sqlx::query_scalar(
        "SELECT b.uuid FROM books b
         JOIN scan_roots l ON l.id = b.library_id
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

/// Delete every per-book link row for `book_id`. Used by `sync_changed`
/// before re-inserting fresh rows — cascade delete on FK isn't an option
/// here (the `books` row stays), so we wipe explicitly. All these tables
/// have UNIQUE(book, ...) constraints, so a re-insert without the wipe
/// would fail.
///
/// `book_files` is scoped to the changed file's own `format`: a blanket
/// wipe would also destroy a cross-format attachment (e.g. the M4B row
/// hanging off this book via `merged_uuids`) every time the ebook
/// re-parses.
async fn wipe_per_book_link_rows(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    format: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM book_files WHERE book_id = ? AND format = ?")
        .bind(book_id)
        .bind(format)
        .execute(&mut **tx)
        .await?;
    for table in &[
        "book_identifiers",
        "books_authors_link",
        "books_tags_link",
        "books_publishers_link",
        "books_series_link",
        "books_languages_link",
    ] {
        // Note: the link tables use `book` (not `book_id`) as the FK
        // column, but `book_identifiers` uses `book_id`. Switch on the
        // table name.
        let col = if *table == "book_identifiers" {
            "book_id"
        } else {
            "book"
        };
        let sql = format!("DELETE FROM {table} WHERE {col} = ?");
        sqlx::query(&sql).bind(book_id).execute(&mut **tx).await?;
    }
    Ok(())
}
