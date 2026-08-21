//! Transactional orchestrator for the indexer write path. Owns
//! `sync_books`, the per-bucket helpers (`sync_removed` / `sync_changed`
//! / `sync_new`), `replace_books`, and post-commit cover materialization.
//! All `books_fts` maintenance is delegated to the [`super::fts`]
//! choke-point (`upsert_fts` / `delete_fts`) rather than written inline.
//! Also owns [`EntityAliasMaps`] and [`collect_entity_alias_maps`], which
//! resolve the reindex-resurrection guard's alias lookups once for the
//! whole New+Changed batch, before either per-book loop starts.

use std::collections::{HashMap, HashSet};

use omnibus_shared::CleanupKind;
use sqlx::{SqlitePool, Transaction};

use crate::covers::delete_cover_files_for;
use crate::entity_alias::resolve_entity_aliases;
use crate::helpers::cleaned_series_name;
use crate::settings::upsert_library;

use super::attach;

mod backfill;
mod changed;
mod moved;
mod new;
mod removed;
mod shared;

#[cfg(test)]
mod tests;

pub(super) use shared::{clear_missing_files_flag, materialize_new_covers};
// Shared with the audiobook pipeline: a relocation is the same write for both
// library kinds (see the `moved` module doc).
pub(super) use moved::sync_moved;

use backfill::stamp_last_indexed;
use changed::sync_changed;
use new::sync_new;
use removed::sync_removed;

/// Crate-internal error wrapping `sqlx::Error` so `?` propagates cleanly in the ebook and audiobook sync helpers.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SyncError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Physical(#[from] crate::physical::PhysicalError),
}

impl From<crate::settings::SettingsError> for SyncError {
    fn from(e: crate::settings::SettingsError) -> Self {
        match e {
            crate::settings::SettingsError::Db(inner) => SyncError::Db(inner),
            // `Validation`/`Json` are only produced by `set_hardcover_api_key`
            // and the F5.1 metadata-precedence writes, which no sync path
            // calls — keep the arms exhaustive but do not widen
            // `SyncError`'s surface for cases the caller graph can't reach.
            crate::settings::SettingsError::Validation(msg) => SyncError::Db(
                sqlx::Error::Protocol(format!("unexpected settings validation error: {msg}")),
            ),
            crate::settings::SettingsError::Json(inner) => SyncError::Db(sqlx::Error::Protocol(
                format!("unexpected settings JSON error: {inner}"),
            )),
        }
    }
}

/// One file the diff recognized as **relocated** rather than
/// deleted-and-re-added. Deliberately not a `ParseTarget`: a move needs no
/// Phase-B parse, and carrying one would invite a caller to hand it to the
/// parser. See the Moved section of the [`crate::indexer::diff_library`]
/// bucket contract.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MovedFile {
    /// The identity to relocate: `books.uuid` for the file a book is anchored
    /// on, the `merged_uuids.uuid` for an attached file. Which one it is
    /// decides whether the book's own `scan_key` moves — see [`sync_moved`].
    pub uuid: String,
    /// The file's new library-relative path — its new `scan_key`, and the
    /// source of the new `books.path` / `book_files.filename`.
    pub filename: String,
}

/// Pre-resolved reindex-resurrection guard (#964) lookups for one whole
/// sync batch: the canonical id every distinct author/series/tag name
/// across the batch resolves to, if a prior library-cleanup merge absorbed
/// it. Built once per [`CleanupKind`] by [`collect_entity_alias_maps`]
/// before the per-book write loop starts, then threaded down through
/// `insert_metadata_links` — replacing what used to be a
/// `resolve_entity_aliases` call issued once per book (#1985).
#[derive(Debug, Default)]
pub(super) struct EntityAliasMaps {
    pub(super) authors: HashMap<String, i64>,
    pub(super) series: HashMap<String, i64>,
    pub(super) tags: HashMap<String, i64>,
}

/// Collect the distinct author, series, and tag names appearing anywhere in
/// `changed_books` or `new_books` — the whole New+Changed batch one
/// [`sync_books_with_progress`] call writes — and resolve each
/// [`CleanupKind`]'s alias guard once for the union, rather than once per
/// book inside the per-bucket write loops.
pub(super) async fn collect_entity_alias_maps(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    changed_books: &[crate::ebook::IndexedBook],
    new_books: &[crate::ebook::IndexedBook],
) -> Result<EntityAliasMaps, sqlx::Error> {
    let mut author_names: HashSet<&str> = HashSet::new();
    let mut series_names: HashSet<String> = HashSet::new();
    let mut tag_names: HashSet<&str> = HashSet::new();

    for b in changed_books.iter().chain(new_books.iter()) {
        let m = &b.metadata;
        author_names.extend(m.creators.iter().map(|c| c.name.as_str()));
        // `cleaned_series_name` returns an owned String (embedded-index
        // stripped, #1912), so the dedup set holds owned values too.
        if let Some(series) = cleaned_series_name(m) {
            series_names.insert(series);
        }
        tag_names.extend(
            m.subjects
                .iter()
                .map(String::as_str)
                .filter(|s| !s.is_empty()),
        );
    }

    let author_list: Vec<&str> = author_names.into_iter().collect();
    let series_list: Vec<&str> = series_names.iter().map(String::as_str).collect();
    let tag_list: Vec<&str> = tag_names.into_iter().collect();

    Ok(EntityAliasMaps {
        authors: resolve_entity_aliases(tx, CleanupKind::Author, &author_list).await?,
        series: resolve_entity_aliases(tx, CleanupKind::Series, &series_list).await?,
        tags: resolve_entity_aliases(tx, CleanupKind::Tag, &tag_list).await?,
    })
}

/// Per-bucket payload for [`sync_books`]. Built by
/// `crate::indexer::diff_library` (plus the Phase-B parse for new + changed).
///
/// The five buckets are mutually exclusive — a given uuid appears in at
/// most one of them per sync — and the diff already ordered them
/// deterministically.
#[derive(Debug, Default)]
pub struct SyncPlan {
    pub new_books: Vec<crate::ebook::IndexedBook>,
    pub changed_books: Vec<crate::ebook::IndexedBook>,
    /// Files that changed path but not content. Applied before `new_books`
    /// so an unrelated new file can't claim a moved file's format slot by
    /// title+author first.
    pub moved: Vec<MovedFile>,
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
/// 3. Relocate Moved files: repoint the path columns of rows that already
///    exist. No parse, no insert, no delete.
/// 4. Update Changed in place (preserves `books.id`); wipe-and-rewrite
///    link rows + FTS row for each.
/// 5. Insert New (autoincrement assigns a fresh id).
/// 6. Backfill: UPDATE `book_files.(mtime_epoch, size_bytes)` only — no
///    OPF re-parse, no link writes, no FTS write. See the Backfill rule
///    in the [`crate::indexer`] module doc.
/// 7. Delete taxonomy rows left with zero books by step 4's link wipe.
/// 8. Stamp `scan_roots.last_indexed`.
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
    sync_books_with_progress(pool, library_path, plan, |_, _, _| {}).await
}

/// [`sync_books`] variant that calls `on_progress(processed, total,
/// current)` after each per-book write so the worker can surface a
/// determinate progress bar naming the book just written (`current` is the
/// library-relative filename; `None` on the initial pre-write tick).
/// `total` is the count of buckets that loop per book — Changed + New.
/// Removed, Moved and Backfill are cheap path-column writes with no parse
/// behind them, so they are not reported as per-book progress (they're
/// invisible to the user-facing "Scanning" step).
pub async fn sync_books_with_progress(
    pool: &SqlitePool,
    library_path: &str,
    plan: SyncPlan,
    mut on_progress: impl FnMut(u32, u32, Option<&str>),
) -> anyhow::Result<()> {
    let total: u32 = (plan.changed_books.len() + plan.new_books.len())
        .try_into()
        .unwrap_or(u32::MAX);
    // Emit an initial (0, total) tick so the UI flips from indeterminate
    // spinner to determinate bar before the first per-book write lands.
    on_progress(0, total, None);

    let mut tx = pool.begin().await?;
    let library_id = upsert_library(&mut tx, library_path).await?;

    sync_removed(&mut tx, library_id, &plan.removed_uuids).await?;
    // Removed uuids that were cross-format attachments have no `books`
    // row — drop their `book_files` row + `merged_uuids` entry instead
    // (the target book survives, possibly fileless).
    attach::remove_attached_files(&mut tx, &plan.removed_uuids).await?;
    // Before New: the move path never drops a `book_files` row, so it can't
    // open an attach slot — but if `sync_new` ran first, an unrelated new file
    // could claim a book by title+author and the moved file would then find
    // its own format slot occupied.
    sync_moved(&mut tx, library_id, library_path, &plan.moved).await?;

    // #1985: resolve the reindex-resurrection guard once for the whole
    // New+Changed batch, before either per-book loop starts, instead of
    // once per book inside them.
    let alias_maps =
        collect_entity_alias_maps(&mut tx, &plan.changed_books, &plan.new_books).await?;

    let mut processed: u32 = 0;
    let changed_covers = sync_changed(
        &mut tx,
        library_id,
        library_path,
        &plan.changed_books,
        &alias_maps,
        |filename| {
            processed = processed.saturating_add(1);
            on_progress(processed, total, Some(filename));
        },
    )
    .await?;
    let new_covers = sync_new(
        &mut tx,
        library_id,
        library_path,
        &plan.new_books,
        &plan.removed_uuids,
        &alias_maps,
        |filename| {
            processed = processed.saturating_add(1);
            on_progress(processed, total, Some(filename));
        },
    )
    .await?;
    super::backfill::backfill_stat_chunks(&mut tx, library_id, &plan.backfill).await?;
    // Changed is the only bucket that drops link rows (`wipe_per_book_link_rows`
    // before the re-insert), so it's the only one that can leave an author or
    // series with zero books. Skip the sweep otherwise — it scans five taxonomy
    // tables and most rescans have no Changed books at all.
    if !plan.changed_books.is_empty() {
        crate::taxonomy::delete_orphan_taxonomy(&mut tx).await?;
    }
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
        tracing::error!(error = %join_err, "sync_books: cover reconcile spawn_blocking failed");
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
            removed_uuids,
            ..Default::default()
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
