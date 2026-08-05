//! SQL CRUD for the `metadata_overrides` table plus the small filesystem
//! helpers (`write_override_cover`, `delete_override_cover`) that share
//! the same write-path callers. Read merge (`apply_overrides`) lives
//! here too because it consumes a row produced by the same SELECT.

use std::collections::HashMap;

use omnibus_shared::{BulkMetadataEdit, EbookMetadata, MetadataOverrides, MetadataSource};
use sqlx::{Executor, SqliteConnection, SqlitePool};

use crate::books::{get_books_by_ids, resolve_book_id_by_uuid_exec, resolve_book_ids_bulk};
use crate::normalize::{normalize_author, normalize_title};
use crate::sync::upsert_fts;

use super::fts::rebuild_fts_for_book;
use super::links::{materialize_genre_rows, materialize_series_link, materialize_tag_rows};

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
    #[error("{field} list for {uuid} would exceed {max} entries")]
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
        }
    }
}

/// Execute the `metadata_overrides` INSERT…ON CONFLICT against any executor —
/// a `&SqlitePool` for the fire-and-forget [`upsert_metadata_overrides`] path,
/// or a transaction connection for the serialized [`merge_metadata_overrides`]
/// path — so the upsert statement lives in exactly one place and the two
/// callers can't drift (e.g. when a column or default changes).
async fn upsert_overrides_row<'e, E>(
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
async fn touch_book_last_modified(
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
async fn invalidate_export_epub_cache_for(book_id: Option<i64>) {
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
/// matching the [`merge_metadata_overrides`] pattern — so a failure in a
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
    let json = serde_json::to_string(overrides)?;

    // `begin_with("BEGIN IMMEDIATE")` matches `merge_metadata_overrides` — the
    // RESERVED lock at start makes this writer wait for any in-flight writer
    // (SQLite is single-writer at the database level), and the returned
    // `sqlx::Transaction` issues a structured ROLLBACK on any `?` early-return
    // below.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    upsert_overrides_row(
        &mut *tx,
        book_uuid,
        overrides,
        &json,
        has_cover_override,
        user_id,
    )
    .await?;
    materialize_series_link(&mut tx, book_uuid, overrides).await?;
    materialize_tag_rows(&mut tx, overrides).await?;
    materialize_genre_rows(&mut tx, overrides).await?;
    touch_book_last_modified(&mut tx, book_uuid).await?;
    // A subjects replacement may have dropped a tag's last membership — reap
    // orphans in the same tx so no surface ever serves a bookless tag.
    crate::taxonomy::delete_orphan_tags(&mut tx).await?;
    crate::taxonomy::delete_orphan_genres(&mut tx).await?;
    tx.commit().await?;

    if let Err(e) = rebuild_fts_for_book(pool, book_uuid).await {
        tracing::warn!(book_uuid, error = %e, "books_fts rebuild after override upsert failed");
    }
    Ok(())
}

/// Merge `incoming` field overrides on top of any existing overrides for
/// `book_uuid` and persist the result inside one `BEGIN IMMEDIATE`
/// transaction, so two concurrent edits to the same book can't interleave
/// and silently drop each other's changes. The existing `has_cover_override`
/// flag is carried forward — a text-only edit must not clear a cover the
/// user uploaded earlier. The series-link and tags-row materializations run
/// inside the same transaction so the override row and its canonical rows
/// land atomically; the `books_fts` rebuild runs best-effort after commit.
pub async fn merge_metadata_overrides(
    pool: &SqlitePool,
    book_uuid: &str,
    incoming: &MetadataOverrides,
    user_id: i64,
) -> Result<(), MetadataOverridesError> {
    // `begin_with("BEGIN IMMEDIATE")` gives the same RESERVED-lock-at-start
    // semantics as the old hand-rolled statement (SQLite is single-writer at
    // the database level, so this writer waits for any in-flight writer
    // before acquiring), but returns a real `sqlx::Transaction`. Any `?`
    // early-return below drops `tx` without a commit, and the Drop impl
    // issues a structured ROLLBACK — no reliance on connection-drop implicit
    // cleanup.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    merge_one_in_tx(&mut tx, book_uuid, incoming, user_id).await?;
    // The merged subjects may have dropped a tag's last membership (m2m
    // fields replace wholesale when `Some`) — reap orphans before commit.
    crate::taxonomy::delete_orphan_tags(&mut tx).await?;
    crate::taxonomy::delete_orphan_genres(&mut tx).await?;
    tx.commit().await?;

    if let Err(e) = rebuild_fts_for_book(pool, book_uuid).await {
        tracing::warn!(book_uuid, error = %e, "books_fts rebuild after override merge failed");
    }
    Ok(())
}

/// The tx-scoped body shared by [`merge_metadata_overrides`] and
/// [`bulk_merge_metadata_overrides`]: read the existing override row, merge
/// `incoming` on top, upsert, materialize the series link, and bump
/// `books.last_modified` — all on the caller's transaction.
async fn merge_one_in_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    book_uuid: &str,
    incoming: &MetadataOverrides,
    user_id: i64,
) -> Result<(), MetadataOverridesError> {
    let existing: Option<(String, i64)> = sqlx::query_as(
        "SELECT overrides, has_cover_override FROM metadata_overrides WHERE book_uuid = ?",
    )
    .bind(book_uuid)
    .fetch_optional(&mut **tx)
    .await?;

    let (merged, has_cover_override) = match existing {
        Some((json, has_cover)) => {
            let prior: MetadataOverrides = serde_json::from_str(&json)?;
            (prior.merge(incoming), has_cover != 0)
        }
        None => (incoming.clone(), false),
    };

    let json = serde_json::to_string(&merged)?;
    upsert_overrides_row(
        &mut **tx,
        book_uuid,
        &merged,
        &json,
        has_cover_override,
        user_id,
    )
    .await?;
    materialize_series_link(tx, book_uuid, &merged).await?;
    materialize_tag_rows(tx, &merged).await?;
    materialize_genre_rows(tx, &merged).await?;
    touch_book_last_modified(tx, book_uuid).await?;
    Ok(())
}

/// Apply one [`BulkMetadataEdit`] to every uuid inside a single
/// `BEGIN IMMEDIATE` transaction — all books update or none do (an unknown
/// uuid fails the whole call with [`MetadataOverridesError::BookNotFound`]).
///
/// Tag deltas are computed per book against its effective subjects: the
/// in-tx override row's `subjects` when one exists, else the merged
/// metadata fetched just before the transaction (equal to the scanned
/// subjects when no override exists — and any concurrent subjects write
/// would have created the override row this transaction reads, so the
/// pre-tx fetch cannot serve a stale base). Both the pre-tx effective-subjects
/// fetch and the in-tx override-row read are batched via chunked `IN (...)`
/// queries (mirroring [`load_overrides_bulk`]) rather than looped per uuid.
///
/// Genre deltas need no such pre-tx fetch: genres exist only in the override
/// row (migration `0066`), so the in-tx read *is* the complete base.
///
/// FTS rebuilds run best-effort per book after commit, matching
/// [`merge_metadata_overrides`].
pub async fn bulk_merge_metadata_overrides(
    pool: &SqlitePool,
    uuids: &[String],
    edit: &BulkMetadataEdit,
    user_id: i64,
) -> Result<(), MetadataOverridesError> {
    let has_tag_deltas = !edit.add_tags.is_empty() || !edit.remove_tags.is_empty();
    let has_genre_deltas = !edit.add_genres.is_empty() || !edit.remove_genres.is_empty();

    // Unconditional, even for an edit with no tag deltas: this is also the
    // call that proves every uuid resolves to a live book, which is what
    // makes an unknown uuid fail the whole batch before the transaction
    // opens rather than half-applying it.
    let effective_subjects = effective_subjects_bulk(pool, uuids).await?;

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    // Pre-loaded once for the whole batch, chunked, instead of a
    // `SELECT … WHERE book_uuid = ?` per uuid inside the loop below.
    let override_rows = if has_tag_deltas || has_genre_deltas {
        load_overrides_bulk_tx(&mut tx, uuids).await?
    } else {
        HashMap::new()
    };
    for uuid in uuids {
        let mut incoming = edit.scalar_overrides();
        if has_tag_deltas {
            // The in-tx override row wins as the tag base; the pre-tx fetch
            // only covers books with no subjects override at all.
            let override_subjects = override_rows
                .get(uuid)
                .and_then(|ov| ov.subjects.as_deref());
            let base = override_subjects
                .or(effective_subjects.get(uuid).map(Vec::as_slice))
                .unwrap_or_default();
            let subjects = edit.apply_tags(base);
            check_list_cap(uuid, "tag", subjects.len(), MetadataOverrides::MAX_SUBJECTS)?;
            incoming.subjects = Some(subjects);
        }
        if has_genre_deltas {
            let base = override_rows
                .get(uuid)
                .and_then(|ov| ov.genres.as_deref())
                .unwrap_or_default();
            let genres = edit.apply_genres(base);
            check_list_cap(uuid, "genre", genres.len(), MetadataOverrides::MAX_GENRES)?;
            incoming.genres = Some(genres);
        }
        merge_one_in_tx(&mut tx, uuid, &incoming, user_id).await?;
    }
    // One sweep for the whole batch, not per book: a `remove_tags` delta can
    // drop a tag's last membership anywhere in the set.
    crate::taxonomy::delete_orphan_tags(&mut tx).await?;
    crate::taxonomy::delete_orphan_genres(&mut tx).await?;
    tx.commit().await?;

    for uuid in uuids {
        if let Err(e) = rebuild_fts_for_book(pool, uuid).await {
            tracing::warn!(book_uuid = %uuid, error = %e, "books_fts rebuild after bulk override merge failed");
        }
    }
    Ok(())
}

/// Reject a post-delta list that would push one book past its per-book cap.
/// The whole bulk call rolls back — a partial apply would leave the batch
/// half-edited with no way for the caller to tell which half.
fn check_list_cap(
    uuid: &str,
    field: &'static str,
    len: usize,
    max: usize,
) -> Result<(), MetadataOverridesError> {
    if len > max {
        return Err(MetadataOverridesError::TooManyValues {
            uuid: uuid.to_string(),
            field,
            max,
        });
    }
    Ok(())
}

/// Batch-resolve the effective (scanned + override-merged) `subjects` for a
/// whole uuid set, for [`bulk_merge_metadata_overrides`]'s pre-transaction
/// tag-delta base. Chunked bulk queries regardless of batch size —
/// [`resolve_book_ids_bulk`] then [`get_books_by_ids`], the same
/// join-in-memory pattern [`super::fts::rebuild_fts_for_books_batch`] uses.
/// A uuid absent from either step (unknown, or resolved to a book row that
/// vanished concurrently) fails the whole call with `BookNotFound`.
async fn effective_subjects_bulk(
    pool: &SqlitePool,
    uuids: &[String],
) -> Result<HashMap<String, Vec<String>>, MetadataOverridesError> {
    let id_map = resolve_book_ids_bulk(pool, uuids).await?;
    let mut ids: Vec<i64> = id_map.values().copied().collect();
    ids.sort_unstable();
    ids.dedup();

    let subjects_by_id: HashMap<i64, Vec<String>> = get_books_by_ids(pool, &ids)
        .await?
        .into_iter()
        .map(|b| (b.id, b.subjects))
        .collect();

    let mut out = HashMap::with_capacity(uuids.len());
    for uuid in uuids {
        let id = id_map
            .get(uuid)
            .copied()
            .ok_or_else(|| MetadataOverridesError::BookNotFound(uuid.clone()))?;
        let subjects = subjects_by_id
            .get(&id)
            .cloned()
            .ok_or_else(|| MetadataOverridesError::BookNotFound(uuid.clone()))?;
        out.insert(uuid.clone(), subjects);
    }
    Ok(out)
}

/// Tx-scoped counterpart to [`load_overrides_bulk`]: batch-read
/// `metadata_overrides.overrides` for a whole uuid set on the caller's open
/// transaction, chunked the same way, so [`bulk_merge_metadata_overrides`]'s
/// per-book tag-delta base comes from one pre-loaded `HashMap` instead of a
/// `SELECT … WHERE book_uuid = ?` per uuid.
async fn load_overrides_bulk_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    uuids: &[String],
) -> Result<HashMap<String, MetadataOverrides>, sqlx::Error> {
    let mut map = HashMap::with_capacity(uuids.len());
    for chunk in uuids.chunks(500) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        let sql = format!(
            "SELECT book_uuid, overrides FROM metadata_overrides WHERE book_uuid IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (String, String)>(&sql);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        let rows = q.fetch_all(&mut **tx).await?;
        for (uuid, json) in rows {
            match serde_json::from_str::<MetadataOverrides>(&json) {
                Ok(ov) => {
                    map.insert(uuid, ov);
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

/// Load overrides for a single book UUID. Returns `None` if no overrides
/// exist.
pub async fn get_metadata_overrides(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<Option<(MetadataOverrides, bool)>, MetadataOverridesError> {
    let row: Option<(String, i64)> = sqlx::query_as(
        "SELECT overrides, has_cover_override FROM metadata_overrides WHERE book_uuid = ?",
    )
    .bind(book_uuid)
    .fetch_optional(pool)
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
/// Unlike [`upsert_metadata_overrides`] / [`merge_metadata_overrides`]
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
    sqlx::query("DELETE FROM metadata_overrides WHERE book_uuid = ?")
        .bind(book_uuid)
        .execute(&mut *tx)
        .await?;
    // Resolve inside the transaction so the id read agrees with the DELETE.
    // A uuid with no live book has no FTS row to restore.
    let book_id = resolve_book_id_by_uuid_exec(&mut *tx, book_uuid).await?;
    if let Some(id) = book_id {
        upsert_fts(&mut tx, id).await?;
    }
    touch_book_last_modified(&mut tx, book_uuid).await?;
    // Reverting to scanned drops every override membership this row held —
    // tags that existed only through it are now orphans.
    crate::taxonomy::delete_orphan_tags(&mut tx).await?;
    crate::taxonomy::delete_orphan_genres(&mut tx).await?;
    tx.commit().await?;

    // The override state is now fully cleared, so any cached rewritten
    // export EPUB has nothing left to bake — remove it rather than leaving
    // it orphaned on disk (#1395).
    invalidate_export_epub_cache_for(book_id).await;
    Ok(())
}

/// Clear a book's cover override, reverting `cover_url` to the scanned
/// original while preserving any text-field overrides that remain. A no-op
/// if the book has no cover override active.
///
/// When no text overrides remain either, the whole `metadata_overrides` row
/// is deleted (mirrors [`delete_metadata_overrides`]'s FTS-restore step) so
/// `has_override` reads false again — otherwise an empty-but-present row
/// would leave the "Override active" indicator stuck on.
pub async fn clear_cover_override(
    pool: &SqlitePool,
    book_uuid: &str,
    user_id: i64,
) -> Result<(), MetadataOverridesError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let existing: Option<(String, i64)> = sqlx::query_as(
        "SELECT overrides, has_cover_override FROM metadata_overrides WHERE book_uuid = ?",
    )
    .bind(book_uuid)
    .fetch_optional(&mut *tx)
    .await?;

    let Some((json, has_cover)) = existing else {
        tx.commit().await?;
        return Ok(());
    };
    if has_cover == 0 {
        tx.commit().await?;
        return Ok(());
    }

    let overrides: MetadataOverrides = serde_json::from_str(&json)?;
    let mut cleared_book_id = None;
    if overrides == MetadataOverrides::default() {
        sqlx::query("DELETE FROM metadata_overrides WHERE book_uuid = ?")
            .bind(book_uuid)
            .execute(&mut *tx)
            .await?;
        // Resolve inside the transaction so the id read agrees with the DELETE.
        cleared_book_id = resolve_book_id_by_uuid_exec(&mut *tx, book_uuid).await?;
        if let Some(book_id) = cleared_book_id {
            upsert_fts(&mut tx, book_id).await?;
        }
    } else {
        upsert_overrides_row(&mut *tx, book_uuid, &overrides, &json, false, user_id).await?;
    }
    touch_book_last_modified(&mut tx, book_uuid).await?;
    tx.commit().await?;

    // Only the "no override left at all" branch above stales the export-EPUB
    // cache — see [`delete_metadata_overrides`]'s equivalent cleanup (#1395).
    // A cover-only clear that leaves text overrides in place still needs a
    // rewrite (just without the cover swap), which the staleness check in
    // `rewritten_epub_path` already handles via the `last_modified` bump.
    invalidate_export_epub_cache_for(cleared_book_id).await;
    Ok(())
}

/// Look up the UUID for a given `books.id`. Used by the override-save
/// endpoints to bridge the id-based API with the uuid-keyed overrides table.
pub async fn get_book_uuid(
    pool: &SqlitePool,
    book_id: i64,
) -> Result<Option<String>, MetadataOverridesError> {
    Ok(sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_optional(pool)
        .await?)
}

/// Bulk-load overrides for a set of UUIDs. Returns a map from UUID to
/// `(overrides, has_cover_override)`. Used by `list_books` and `search_books`
/// to merge overrides without N+1 queries.
pub(crate) async fn load_overrides_bulk(
    pool: &SqlitePool,
    uuids: &[String],
) -> Result<HashMap<String, (MetadataOverrides, bool)>, sqlx::Error> {
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

/// Apply a `MetadataOverrides` to an `EbookMetadata`, mutating it in place,
/// gated by whether `precedence` (the owning scan root's configured
/// metadata-source order) ranks `OmnibusOverrides` above
/// `EmbeddedTags` — the two sources with a real data provider today (see
/// [`overrides_outrank_embedded`]). Scalar fields are replaced when `Some`;
/// m2m fields (`creators`, `subjects`, `genres`) replace entirely when
/// present. `genres` has no scanned counterpart — this is the only place a
/// book's genres are ever populated.
pub(crate) fn apply_overrides(
    book: &mut EbookMetadata,
    uuid: &str,
    ov: &MetadataOverrides,
    has_cover_override: bool,
    precedence: &[MetadataSource],
) {
    if !overrides_outrank_embedded(precedence) {
        return;
    }
    if let Some(ref t) = ov.title {
        book.title = Some(t.clone());
    }
    if let Some(ref d) = ov.description {
        book.description = crate::books::sanitize_description(Some(d.clone()));
    }
    if let Some(ref p) = ov.publisher {
        book.publisher = Some(p.clone());
    }
    if let Some(ref d) = ov.published {
        book.published = Some(d.clone());
    }
    if let Some(ref l) = ov.language {
        book.language = Some(l.clone());
    }
    if let Some(ref s) = ov.series {
        book.series = Some(s.clone());
    }
    if let Some(ref si) = ov.series_index {
        book.series_index = Some(si.clone());
    }
    if let Some(ref i) = ov.isbn13 {
        // Empty string is the clear sentinel (mirrors `build_overrides` on
        // the frontend) — an override that clears the field must read back
        // as "no ISBN", not as a literal empty string.
        book.isbn13 = if i.is_empty() { None } else { Some(i.clone()) };
    }
    if let Some(ref c) = ov.creators {
        book.creators = c.clone();
    }
    if let Some(ref s) = ov.subjects {
        book.subjects = s.clone();
    }
    if let Some(ref g) = ov.genres {
        book.genres = g.clone();
    }
    book.has_cover_override = has_cover_override;
    if has_cover_override {
        // Ensure cover_url is set even if the original had no cover. The
        // REST route is uuid-keyed (`/api/covers/{uuid}`), matching the
        // non-override cover_url construction in books.rs — never `book.id`.
        book.cover_url = Some(format!("/api/covers/{uuid}"));
    }
    book.has_override = true;
}

/// Whether `MetadataSource::OmnibusOverrides` should win over
/// `MetadataSource::EmbeddedTags` (the scanned baseline) under a library's
/// configured precedence order. List order is lowest-to-highest priority,
/// so overrides win when they appear *after* embedded tags.
/// `FolderStructure`/`OpfSidecar`/`ProviderMatch` have no data provider yet
/// (see [`apply_overrides`]'s doc comment), so their position in the list
/// doesn't currently affect this.
///
/// A malformed/partial list missing one of the two real sources falls back
/// to the legacy always-wins behavior rather than silently dropping
/// overrides.
fn overrides_outrank_embedded(precedence: &[MetadataSource]) -> bool {
    let embedded = precedence
        .iter()
        .position(|s| *s == MetadataSource::EmbeddedTags);
    let overrides = precedence
        .iter()
        .position(|s| *s == MetadataSource::OmnibusOverrides);
    match (embedded, overrides) {
        (Some(e), Some(o)) => o > e,
        _ => true,
    }
}

/// Write a user-uploaded override cover to disk.
pub fn write_override_cover(
    uuid: &str,
    mime: &str,
    bytes: &[u8],
) -> Result<(), MetadataOverridesError> {
    let ext = crate::covers::ImageFormat::from_mime(mime).to_ext();
    let dir = crate::covers::covers_dir();
    std::fs::create_dir_all(&dir)?;

    // Remove any existing override cover with a different extension. A
    // missing file is the expected/common case (most probed extensions
    // won't exist) and is ignored; any other failure (e.g. permissions)
    // must propagate — silently leaving a stale file behind would let the
    // extension probe in `find_override_cover_file` keep serving it instead
    // of the cover just written below.
    for fmt in crate::covers::ImageFormat::PROBE_ORDER {
        let old = dir.join(format!("override-{uuid}.{}", fmt.to_ext()));
        match std::fs::remove_file(old) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }

    std::fs::write(dir.join(format!("override-{uuid}.{ext}")), bytes)?;
    Ok(())
}

/// Delete override cover files for a UUID.
pub fn delete_override_cover(uuid: &str) {
    let dir = crate::covers::covers_dir();
    for fmt in crate::covers::ImageFormat::PROBE_ORDER {
        let _ = std::fs::remove_file(dir.join(format!("override-{uuid}.{}", fmt.to_ext())));
    }
}
