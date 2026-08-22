//! SQL CRUD for the `metadata_overrides` table: the error enum, the
//! shared row-write/last-modified/cache-invalidation helpers, single-book
//! upsert, plain get/delete, and the bulk-read helper `list_books`/
//! `search_books` join against. Called by `db::metadata_overrides` and,
//! through it, the settings and books RPC/REST handlers.

use std::collections::HashMap;

use omnibus_shared::MetadataOverrides;
use sqlx::{Executor, SqliteConnection, SqlitePool};

use crate::books::resolve_book_id_by_uuid_exec;
use crate::normalize::{normalize_author, normalize_title};
use crate::sync::upsert_fts;

use super::fts::rebuild_fts_for_book;
use super::links::{materialize_genre_rows, materialize_series_link, materialize_tag_rows};

mod cover;
mod merge;

pub(crate) use cover::apply_overrides;
pub use cover::{clear_cover_override, delete_override_cover, write_override_cover};
pub use merge::{bulk_merge_metadata_overrides, merge_metadata_overrides};

/// Errors returned by the metadata overrides data layer.
#[derive(Debug, thiserror::Error)]
pub enum MetadataOverridesError {
    /// A JSON serialization or deserialization failure on the `overrides` blob.
    #[error("JSON (de)serialization failed: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    /// A bulk edit named a uuid with no live book row — the whole bulk
    /// call rolls back rather than applying to a subset.
    #[error("book not found: {0}")]
    BookNotFound(String),
    /// A bulk add/remove delta would push one book past a per-book list
    /// cap. `field` names the list ("tag", "genre") — one variant rather
    /// than one per list, since no caller branches on which it was.
    #[error("{field} list for {uuid} would exceed {max} {field}s")]
    TooManyValues {
        uuid: String,
        field: &'static str,
        max: usize,
    },
}

impl From<crate::books::BooksError> for MetadataOverridesError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => MetadataOverridesError::Db(inner),
            crate::books::BooksError::OverridesJson(inner) => {
                MetadataOverridesError::Serialization(inner)
            }
            // A `BooksError` this module can receive never carries one: the
            // variant exists for the overrides *read* path's fold-through.
            crate::books::BooksError::Other(msg) => {
                MetadataOverridesError::Db(sqlx::Error::Decode(msg.into()))
            }
        }
    }
}

/// Execute the `metadata_overrides` INSERT…ON CONFLICT against any executor —
/// a `&SqlitePool` for the fire-and-forget [`upsert_metadata_overrides`] path,
/// or a transaction connection for the serialized [`merge::merge_metadata_overrides`]
/// path — so the upsert statement lives in exactly one place and the two
/// callers can't drift (e.g. when a column or default changes).
pub(super) async fn upsert_overrides_row<'e, E>(
    executor: E,
    book_uuid: &str,
    overrides: &MetadataOverrides,
    overrides_json: &str,
    has_cover_override: bool,
    user_id: i64,
) -> Result<(), sqlx::Error>
where
    E: Executor<'e, Database = sqlx::Sqlite>,
{
    // Effective match keys for Physical Check-In's fuzzy rung — derived from the
    // same overrides we store so the columns never drift from them. NULL when the
    // override doesn't set that field, so the resolver falls back to the scanned
    // `books.*_norm` (see migration 0048). Computed from the typed struct the
    // caller already holds, so the write path never re-parses the JSON.
    let (title_norm, author_norm) = override_match_keys(overrides);
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides, has_cover_override, updated_by, updated_at, title_norm, author_norm)
         VALUES (?, ?, ?, ?, strftime('%s','now'), ?, ?)
         ON CONFLICT(book_uuid) DO UPDATE SET
           overrides = excluded.overrides,
           has_cover_override = excluded.has_cover_override,
           updated_by = excluded.updated_by,
           updated_at = strftime('%s','now'),
           title_norm = excluded.title_norm,
           author_norm = excluded.author_norm",
    )
    .bind(book_uuid)
    .bind(overrides_json)
    .bind(i64::from(has_cover_override))
    .bind(user_id)
    .bind(title_norm)
    .bind(author_norm)
    .execute(executor)
    .await?;
    Ok(())
}

/// Bump `books.last_modified` for `book_uuid` (merged-uuid aware) inside the
/// caller's transaction. `last_modified` is the cache-invalidation clock for
/// the thumbnail, KEPUB, and export-EPUB caches, so every override write must
/// touch it — otherwise a title/cover edit leaves those caches serving the
/// pre-edit file. A uuid with no live book row is a no-op.
pub(super) async fn touch_book_last_modified(
    conn: &mut SqliteConnection,
    book_uuid: &str,
) -> Result<(), sqlx::Error> {
    // Resolve uuid→id (merged-uuid aware, same UNION as
    // `resolve_book_id_by_uuid_exec`) inline so the write stays on `sqlx::Error`
    // — a bare id lookup can't produce the `BooksError::OverridesJson` variant,
    // so routing through the typed resolver would only add a misleading map_err.
    // A uuid with no live book row updates zero rows (a no-op).
    sqlx::query(
        "UPDATE books SET last_modified = strftime('%s','now')
         WHERE id = (
             SELECT id FROM books WHERE uuid = ?1
             UNION ALL
             SELECT book_id FROM merged_uuids WHERE uuid = ?1
             LIMIT 1
         )",
    )
    .bind(book_uuid)
    .execute(&mut *conn)
    .await?;
    Ok(())
}

/// Best-effort removal of a book's cached export-EPUB after its override
/// state clears entirely. No-op when `book_id` is `None` (the uuid had no
/// live book row to resolve). Runs on the blocking pool since
/// `invalidate_export_epub_cache` is a sync `std::fs` call; a join failure
/// is logged and swallowed — a stray cache file is a disk-space nuisance,
/// not a correctness problem (#1395).
pub(super) async fn invalidate_export_epub_cache_for(book_id: Option<i64>) {
    let Some(book_id) = book_id else { return };
    if let Err(e) = tokio::task::spawn_blocking(move || {
        crate::epub_rewrite::invalidate_export_epub_cache(book_id)
    })
    .await
    {
        tracing::warn!(book_id, error = %e, "export-epub cache invalidate task join failed");
    }
}

/// Normalized effective (title, author) match keys for a set of overrides,
/// mirroring how the sync writers derive `books.(title_norm, author_norm)` from
/// scanned metadata: `normalize_title` of the override title, `normalize_author`
/// of the first override creator. Either side is `None` when the override
/// doesn't set that field (or it normalizes to empty), signalling the resolver
/// to fall back to the scanned norm.
pub(super) fn override_match_keys(ov: &MetadataOverrides) -> (Option<String>, Option<String>) {
    let title_norm = ov.title.as_deref().and_then(normalize_title);
    let author_norm = ov
        .creators
        .as_ref()
        .and_then(|c| c.first())
        .and_then(|c| normalize_author(&c.name));
    (title_norm, author_norm)
}

/// Backfill `metadata_overrides.(title_norm, author_norm)` for rows written
/// before migration 0048, and self-heal any row whose stored keys disagree with
/// its current `overrides` JSON. Recomputes from each row's blob and writes only
/// the rows that differ, so it's a no-op once caught up. The table holds one row
/// per manually-edited book (small), so a full scan each boot is cheap.
pub(crate) async fn backfill_override_norm_columns(
    pool: &SqlitePool,
) -> Result<(), MetadataOverridesError> {
    let rows: Vec<(String, String, Option<String>, Option<String>)> = sqlx::query_as(
        "SELECT book_uuid, overrides, title_norm, author_norm FROM metadata_overrides",
    )
    .fetch_all(pool)
    .await?;

    let stale: Vec<(String, Option<String>, Option<String>)> = rows
        .into_iter()
        .filter_map(|(uuid, json, stored_title, stored_author)| {
            // A corrupt blob must not overwrite existing keys with NULL — skip it
            // (mirroring `load_overrides_bulk`) so the data problem is logged, not
            // silently laundered into a degraded match key.
            let ov: MetadataOverrides = match serde_json::from_str(&json) {
                Ok(ov) => ov,
                Err(e) => {
                    tracing::warn!(
                        book_uuid = %uuid,
                        error = %e,
                        "corrupt metadata_overrides JSON — skipping norm backfill for row"
                    );
                    return None;
                }
            };
            let (title_norm, author_norm) = override_match_keys(&ov);
            (title_norm != stored_title || author_norm != stored_author).then_some((
                uuid,
                title_norm,
                author_norm,
            ))
        })
        .collect();
    if stale.is_empty() {
        return Ok(());
    }

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    for (uuid, title_norm, author_norm) in &stale {
        sqlx::query(
            "UPDATE metadata_overrides SET title_norm = ?, author_norm = ? WHERE book_uuid = ?",
        )
        .bind(title_norm)
        .bind(author_norm)
        .bind(uuid)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Upsert user metadata overrides for a book identified by its stable UUID.
/// The `overrides` are JSON-serialized into the `metadata_overrides` table.
///
/// The override row write, the `books_series_link` materialization, and the
/// `tags`-row materialization run inside one `BEGIN IMMEDIATE` transaction —
/// matching the [`merge::merge_metadata_overrides`] pattern — so a failure in a
/// materialize rolls back the override row instead of leaving the book
/// detail page reading a fresh override against a stale link.
///
/// The `books_fts` rebuild runs best-effort after commit: a stale FTS row
/// is recoverable (next reindex / next save corrects it) and is far less
/// user-visible than a stale series association.
pub async fn upsert_metadata_overrides(
    pool: &SqlitePool,
    book_uuid: &str,
    overrides: &MetadataOverrides,
    has_cover_override: bool,
    user_id: i64,
) -> Result<(), MetadataOverridesError> {
    // `begin_with("BEGIN IMMEDIATE")` matches `merge_metadata_overrides` — the
    // RESERVED lock at start makes this writer wait for any in-flight writer
    // (SQLite is single-writer at the database level), and the returned
    // `sqlx::Transaction` issues a structured ROLLBACK on any `?` early-return
    // below.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    upsert_one_in_tx(&mut tx, book_uuid, overrides, has_cover_override, user_id).await?;
    tx.commit().await?;

    if let Err(e) = rebuild_fts_for_book(pool, book_uuid).await {
        tracing::warn!(book_uuid, error = %e, "books_fts rebuild after override upsert failed");
    }
    Ok(())
}

/// Tx-scoped body of [`upsert_metadata_overrides`]: the override row write,
/// series/tag/genre materialization, `last_modified` touch, and orphan reap
/// — everything but the transaction's own begin/commit and the post-commit
/// best-effort FTS rebuild, both left to the caller. `pub(crate)` so a
/// caller that already holds an open transaction (the library-cleanup
/// apply/undo primitives in [`crate::cleanup`]) can join it instead of
/// nesting a second `BEGIN IMMEDIATE`, which SQLite's single-writer model
/// would otherwise serialize behind the caller's own lock.
pub(crate) async fn upsert_one_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    book_uuid: &str,
    overrides: &MetadataOverrides,
    has_cover_override: bool,
    user_id: i64,
) -> Result<(), MetadataOverridesError> {
    let json = serde_json::to_string(overrides)?;
    upsert_overrides_row(
        &mut **tx,
        book_uuid,
        overrides,
        &json,
        has_cover_override,
        user_id,
    )
    .await?;
    materialize_series_link(tx, book_uuid, overrides).await?;
    materialize_tag_rows(tx, overrides).await?;
    materialize_genre_rows(tx, overrides).await?;
    touch_book_last_modified(tx, book_uuid).await?;
    // A subjects replacement may have dropped a tag's last membership — reap
    // orphans in the same tx so no surface ever serves a bookless tag.
    crate::taxonomy::delete_orphan_tags(tx).await?;
    crate::taxonomy::delete_orphan_genres(tx).await?;
    Ok(())
}

/// Load overrides for a single book UUID. Returns `None` if no overrides
/// exist.
pub async fn get_metadata_overrides(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<Option<(MetadataOverrides, bool)>, MetadataOverridesError> {
    get_metadata_overrides_exec(pool, book_uuid).await
}

/// Executor-generic body of [`get_metadata_overrides`], so a caller that
/// already holds an open transaction (e.g. [`crate::cleanup::apply::apply_book_title_override`])
/// can read the current overrides through that same transaction instead of
/// racing it against a separate pool-level read taken before the
/// transaction started.
pub(crate) async fn get_metadata_overrides_exec<'e, E>(
    executor: E,
    book_uuid: &str,
) -> Result<Option<(MetadataOverrides, bool)>, MetadataOverridesError>
where
    E: sqlx::Executor<'e, Database = sqlx::Sqlite>,
{
    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT overrides, has_cover_override FROM metadata_overrides WHERE book_uuid = ?",
    )
    .bind(book_uuid)
    .fetch_optional(executor)
    .await?;

    match row {
        Some((json, has_cover)) => {
            let ov: MetadataOverrides = serde_json::from_str(&json)?;
            Ok(Some((ov, has_cover != 0)))
        }
        None => Ok(None),
    }
}

/// Delete overrides for a book UUID (revert to scanned values).
///
/// The row DELETE and the `books_fts` restore run inside one
/// `BEGIN IMMEDIATE` transaction. With the override row removed, the
/// canonical [`upsert_fts`] write inside the transaction *is* the
/// revert-to-scanned state — no override overlay is needed, and it reads
/// the same canonical taxonomy the scan-time index writes.
///
/// Unlike [`upsert_metadata_overrides`] / [`merge::merge_metadata_overrides`]
/// (whose post-commit FTS rebuild is best-effort), the FTS restore here is
/// **not** best-effort: a failure in the restore aborts the whole call and
/// rolls the DELETE back, so the overrides can never be dropped while
/// search still matches the deleted override text. Callers must treat this
/// as fallible — the delete can fail (surfacing as a 500 at the handler)
/// even when the row existed, leaving the override intact.
pub async fn delete_metadata_overrides(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<(), MetadataOverridesError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    let book_id = delete_one_in_tx(&mut tx, book_uuid).await?;
    tx.commit().await?;

    // The override state is now fully cleared, so any cached rewritten
    // export EPUB has nothing left to bake — remove it rather than leaving
    // it orphaned on disk (#1395).
    invalidate_export_epub_cache_for(book_id).await;
    Ok(())
}

/// Tx-scoped body of [`delete_metadata_overrides`]: the row DELETE, the
/// `books_fts` restore, the `last_modified` touch, and orphan reap — all on
/// the caller's transaction, mirroring [`upsert_one_in_tx`]. Returns the
/// resolved book id (`None` if the uuid had no live book row) so a
/// pool-level caller can still run the post-commit export-EPUB cache
/// invalidation; a caller joining an existing transaction (the
/// library-cleanup undo primitive) is free to ignore it, since that
/// eviction is a disk-space nicety rather than a correctness requirement —
/// the cache is keyed on `books.last_modified`, already bumped here.
pub(crate) async fn delete_one_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    book_uuid: &str,
) -> Result<Option<i64>, MetadataOverridesError> {
    sqlx::query("DELETE FROM metadata_overrides WHERE book_uuid = ?")
        .bind(book_uuid)
        .execute(&mut **tx)
        .await?;
    // Resolve inside the transaction so the id read agrees with the DELETE.
    // A uuid with no live book has no FTS row to restore.
    let book_id = resolve_book_id_by_uuid_exec(&mut **tx, book_uuid).await?;
    if let Some(id) = book_id {
        upsert_fts(tx, id).await?;
    }
    touch_book_last_modified(tx, book_uuid).await?;
    // Reverting to scanned drops every override membership this row held —
    // tags that existed only through it are now orphans.
    crate::taxonomy::delete_orphan_tags(tx).await?;
    crate::taxonomy::delete_orphan_genres(tx).await?;
    Ok(book_id)
}

/// Bulk-load overrides for a set of UUIDs. Returns a map from UUID to
/// `(overrides, has_cover_override)`. Used by `list_books` and `search_books`
/// to merge overrides without N+1 queries.
pub(crate) async fn load_overrides_bulk(
    pool: &SqlitePool,
    uuids: &[String],
) -> Result<HashMap<String, (MetadataOverrides, bool)>, MetadataOverridesError> {
    if uuids.is_empty() {
        return Ok(HashMap::new());
    }

    // SQLite has a limit on bound parameters (999 by default). For very large
    // libraries we chunk, but typical libraries have < 10k books.
    let mut map = HashMap::with_capacity(uuids.len());

    for chunk in uuids.chunks(500) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT book_uuid, overrides, has_cover_override FROM metadata_overrides WHERE book_uuid IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (String, String, i64)>(&sql);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        let rows = q.fetch_all(pool).await?;
        for (uuid, json, has_cover) in rows {
            match serde_json::from_str::<MetadataOverrides>(&json) {
                Ok(ov) => {
                    map.insert(uuid, (ov, has_cover != 0));
                }
                Err(e) => {
                    tracing::warn!(
                        book_uuid = %uuid,
                        error = %e,
                        "corrupt metadata_overrides JSON — skipping row"
                    );
                }
            }
        }
    }

    Ok(map)
}
