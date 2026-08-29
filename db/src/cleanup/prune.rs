//! Retiring pending `dedup_suggestions` rows that have stopped describing the
//! library. Detection is idempotent but never subtractive on its own — a row
//! written once stays pending until somebody decides it — so a suggestion
//! outlives the thing it names and the queue keeps offering to delete authors
//! that are already gone.

use omnibus_shared::{CleanupAction, CleanupKind};
use sqlx::SqlitePool;

use super::{DetectedSuggestion, MergeEntity};

/// Delete every pending suggestion whose target no longer exists.
///
/// This is the cheap half of staleness — "the row it names is gone" — and it
/// is deliberately a read-path concern: an admin who deletes an author from
/// the author page never runs detection, so without a self-healing pass here
/// the dashboard would keep counting that author's card until they did.
/// Decided rows are left alone; they are the ledger that stops detection
/// re-suggesting what was already reviewed. So is a row whose payload this
/// build cannot read: every clause below requires the key it keys on to be
/// present, so an undecodable row reaches [`super::review_queue`] and is
/// *reported* rather than quietly destroyed by a prune that could not tell
/// what it was deleting.
///
/// Returns how many rows were retired.
pub async fn prune_stale_suggestions(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    let mut removed = 0;
    for entity in [MergeEntity::Author, MergeEntity::Series, MergeEntity::Tag] {
        removed += prune_missing_merge(pool, entity).await?;
    }
    removed += prune_missing_single(pool, CleanupKind::Tag, CleanupAction::Split, "tags").await?;
    removed +=
        prune_missing_single(pool, CleanupKind::Author, CleanupAction::Delete, "authors").await?;
    removed += prune_stale_renames(pool).await?;
    Ok(removed)
}

/// Retire merge suggestions with no survivor to merge into, or nothing left to
/// merge: the canonical row is gone, or every source is.
///
/// Deliberately *not* "any source is gone". A group that has lost one of three
/// duplicates is still a real merge of the two that remain, and the apply
/// primitive already tolerates an id with no rows behind it — pruning on the
/// first missing source would throw away a suggestion that still works. The
/// common case this does catch is the two-way merge whose one source has been
/// deleted, which is what leaves the queue offering to merge away an author
/// that is already gone.
///
/// `table` is [`MergeEntity`]'s own constant, never caller text.
async fn prune_missing_merge(pool: &SqlitePool, entity: MergeEntity) -> Result<u64, sqlx::Error> {
    let table = entity.table();
    let sql = format!(
        "DELETE FROM dedup_suggestions
          WHERE decision = 'pending' AND kind = ? AND action = 'merge'
            AND json_extract(payload_json, '$.canonical_id') IS NOT NULL
            AND (NOT EXISTS (SELECT 1 FROM {table} t
                              WHERE t.id = json_extract(payload_json, '$.canonical_id'))
                 OR NOT EXISTS (SELECT 1 FROM json_each(payload_json, '$.source_ids') s
                                 WHERE EXISTS (SELECT 1 FROM {table} t WHERE t.id = s.value)))"
    );
    Ok(sqlx::query(&sql)
        .bind(entity.kind().as_str())
        .execute(pool)
        .await?
        .rows_affected())
}

/// Retire a split or delete suggestion whose one entity is gone. Both payload
/// shapes name exactly one row, under different keys.
///
/// `table` is a module constant chosen by the caller alongside `kind`, never
/// text from outside this file.
async fn prune_missing_single(
    pool: &SqlitePool,
    kind: CleanupKind,
    action: CleanupAction,
    table: &'static str,
) -> Result<u64, sqlx::Error> {
    let id_key = match action {
        CleanupAction::Split => "$.source_id",
        _ => "$.entity_id",
    };
    let sql = format!(
        "DELETE FROM dedup_suggestions
          WHERE decision = 'pending' AND kind = ? AND action = ?
            AND json_extract(payload_json, '{id_key}') IS NOT NULL
            AND NOT EXISTS (SELECT 1 FROM {table} t
                             WHERE t.id = json_extract(payload_json, '{id_key}'))"
    );
    Ok(sqlx::query(&sql)
        .bind(kind.as_str())
        .bind(action.as_str())
        .execute(pool)
        .await?
        .rows_affected())
}

/// Retire a rename whose book has gone, has been retitled underneath it, or
/// now carries a title override.
///
/// The override clause is the one that matters in practice: accepting a rename
/// *writes* an override, and so does the metadata edit form, so any book whose
/// title a human has settled must stop being offered a proposal derived from
/// the scanned title it still carries on disk.
async fn prune_stale_renames(pool: &SqlitePool) -> Result<u64, sqlx::Error> {
    Ok(sqlx::query(
        "DELETE FROM dedup_suggestions
          WHERE decision = 'pending' AND kind = 'book_title' AND action = 'rename'
            AND json_extract(payload_json, '$.book_uuid') IS NOT NULL
            AND NOT EXISTS (
              SELECT 1 FROM books b
                LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
               WHERE b.uuid = json_extract(payload_json, '$.book_uuid')
                 AND b.title = json_extract(payload_json, '$.current_title')
                 AND json_type(mo.overrides, '$.title') IS NULL)",
    )
    .execute(pool)
    .await?
    .rows_affected())
}

/// Delete pending suggestions of the kinds a detection pass just covered that
/// the pass did not re-emit.
///
/// This is the other half of staleness, and the half [`prune_stale_suggestions`]
/// cannot see: a suggestion whose rows all still exist but which detection no
/// longer considers real — an author every one of whose books has since had its
/// creators overridden away, say. Only the detector knows that, and it only
/// knows it about the kinds it just ran, so `kinds` scopes the delete to those.
pub async fn prune_undetected(
    pool: &SqlitePool,
    kinds: &[CleanupKind],
    fresh: &[DetectedSuggestion],
) -> Result<u64, sqlx::Error> {
    let mut removed = 0;
    for kind in kinds {
        let payloads: Vec<String> = fresh
            .iter()
            .filter(|s| s.kind == *kind)
            .filter_map(|s| serde_json::to_string(&s.payload).ok())
            .collect();
        // An empty array is the honest answer, not a reason to skip: a kind
        // that detects nothing at all must retire its whole pending backlog.
        let json = serde_json::to_string(&payloads).unwrap_or_else(|_| "[]".to_string());
        removed += sqlx::query(
            "DELETE FROM dedup_suggestions
              WHERE decision = 'pending' AND kind = ?
                AND payload_json NOT IN (SELECT value FROM json_each(?))",
        )
        .bind(kind.as_str())
        .bind(json)
        .execute(pool)
        .await?
        .rows_affected();
    }
    Ok(removed)
}

#[cfg(test)]
mod tests;
