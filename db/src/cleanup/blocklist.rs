//! Management surface for the `ignored_authors` blocklist the delete-author
//! flows write: list the entries, convert one into an `entity_aliases`
//! mapping (the recovery path for a duplicate spelling deleted as junk), or
//! remove one outright. The convert is a `cleanup_log`-backed primitive so
//! [`super::undo::undo`] can reverse it exactly.

use omnibus_shared::{CleanupAction, CleanupKind, IgnoredAuthor};
use sqlx::SqlitePool;

use super::apply::CleanupApplyError;
use super::entity_ops::{
    fetch_entity_alias, fetch_name_sort, write_cleanup_log, write_entity_alias,
};
use super::snapshot::{AliasIgnoredSnapshot, AliasSnapshot};
use super::MergeEntity;

#[cfg(test)]
mod tests;

/// Every `ignored_authors` entry, alphabetical by name, for the Settings
/// blocklist list.
pub async fn list_ignored_authors(
    pool: &SqlitePool,
) -> Result<Vec<IgnoredAuthor>, CleanupApplyError> {
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT name, ignored_at FROM ignored_authors ORDER BY name")
            .fetch_all(pool)
            .await?;
    Ok(rows
        .into_iter()
        .map(|(name, ignored_at)| IgnoredAuthor { name, ignored_at })
        .collect())
}

/// Convert the `ignored_authors` entry for `name` into an author
/// `entity_aliases` row pointing at `canonical_id`, so future reindexes
/// resolve the spelling to the canonical author instead of silently skipping
/// it. Errs [`CleanupApplyError::InvalidRequest`] when `name` is not
/// blocklisted and [`CleanupApplyError::NotFound`] when `canonical_id` names
/// no author. Logged in `cleanup_log` (kind Author, action Alias) and
/// undoable.
///
/// Only the *future* mapping is written here — books already indexed while
/// the name was blocklisted keep their missing link until a relink pass
/// re-parses them (`crate::worker::Task::RelinkAuthorless`); callers that
/// want the repair post that task after this returns.
pub async fn apply_alias_ignored_author(
    pool: &SqlitePool,
    name: &str,
    canonical_id: i64,
    applied_by: Option<i64>,
) -> Result<i64, CleanupApplyError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // NOCASE lookup matching the column's own collation, but snapshot the
    // stored casing so undo restores the row exactly as it was.
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT name, ignored_at FROM ignored_authors WHERE name = ?")
            .bind(name)
            .fetch_optional(&mut *tx)
            .await?;
    let Some((stored_name, ignored_at)) = row else {
        return Err(CleanupApplyError::InvalidRequest(format!(
            "author name is not blocklisted: {name}"
        )));
    };
    fetch_name_sort(&mut tx, MergeEntity::Author, canonical_id)
        .await?
        .ok_or(CleanupApplyError::NotFound(canonical_id))?;

    let previous_alias = fetch_entity_alias(&mut tx, CleanupKind::Author, &stored_name)
        .await?
        .map(|(canonical_id, created_at)| AliasSnapshot {
            canonical_id,
            created_at,
        });

    sqlx::query("DELETE FROM ignored_authors WHERE name = ?")
        .bind(&stored_name)
        .execute(&mut *tx)
        .await?;
    write_entity_alias(&mut tx, CleanupKind::Author, &stored_name, canonical_id).await?;

    let snapshot = AliasIgnoredSnapshot {
        name: stored_name,
        ignored_at,
        previous_alias,
    };
    let log_id = write_cleanup_log(
        &mut tx,
        None,
        CleanupKind::Author,
        CleanupAction::Alias,
        &snapshot,
        applied_by,
    )
    .await?;

    tx.commit().await?;
    Ok(log_id)
}

/// Remove the `ignored_authors` entry for `name` outright, so future
/// reindexes may recreate the author from file metadata again. Errs
/// [`CleanupApplyError::InvalidRequest`] when `name` is not blocklisted.
/// Not logged: unlike the convert, the removed row is fully described by its
/// name and re-blocklisting is one delete-author away.
pub async fn remove_ignored_author(pool: &SqlitePool, name: &str) -> Result<(), CleanupApplyError> {
    let result = sqlx::query("DELETE FROM ignored_authors WHERE name = ?")
        .bind(name)
        .execute(pool)
        .await?;
    if result.rows_affected() == 0 {
        return Err(CleanupApplyError::InvalidRequest(format!(
            "author name is not blocklisted: {name}"
        )));
    }
    Ok(())
}
