//! Merge write paths: field-level merge of incoming overrides on top of
//! whatever a book already has, single-book and bulk. Shares its row
//! write, `last_modified` touch, and series/tag/genre materialization
//! with [`super::upsert_metadata_overrides`] rather than duplicating them.

use std::collections::HashMap;

use omnibus_shared::{BulkMetadataEdit, MetadataOverrides};
use sqlx::SqlitePool;

use crate::books::{get_books_by_ids, resolve_book_ids_bulk};

use super::super::fts::{rebuild_fts_for_book, rebuild_fts_for_books_batch};
use super::super::links::{materialize_genre_rows, materialize_series_link, materialize_tag_rows};
use super::{touch_book_last_modified, upsert_overrides_row, MetadataOverridesError};

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
/// queries (mirroring [`super::load_overrides_bulk`]) rather than looped per uuid.
///
/// Genre deltas need no such pre-tx fetch: genres exist only in the override
/// row (migration `0066`), so the in-tx read *is* the complete base.
///
/// FTS rebuilds run best-effort for the whole batch after commit via
/// [`rebuild_fts_for_books_batch`], one write-lock acquisition instead of
/// one per book.
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

    if let Err(e) = rebuild_fts_for_books_batch(pool, uuids).await {
        tracing::warn!(book_count = uuids.len(), error = %e, "books_fts batch rebuild after bulk override merge failed");
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
/// join-in-memory pattern [`super::super::fts::rebuild_fts_for_books_batch`]
/// uses. A uuid absent from either step (unknown, or resolved to a book row
/// that vanished concurrently) fails the whole call with `BookNotFound`.
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

/// Tx-scoped counterpart to [`super::load_overrides_bulk`]: batch-read
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
