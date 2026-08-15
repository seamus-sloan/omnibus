//! Reverses an applied cleanup by replaying the `cleanup_log.snapshot_json`
//! a `super::apply` primitive wrote. Mirrors `apply` action-for-action —
//! any drift between the two shows up as a failing round-trip test rather
//! than a silent partial undo.

use std::collections::HashSet;

use omnibus_shared::{CleanupAction, CleanupKind};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::sync::upsert_fts;
use crate::taxonomy::delete_orphan_taxonomy;

use super::apply::CleanupApplyError;
use super::entity_ops::{
    clear_sort, delete_entity_alias, delete_link, insert_link, lookup_tag_id, recreate_entity,
    restore_canonical_photo, write_photo,
};
use super::snapshot::{DeleteAuthorSnapshot, MergeSnapshot, RenameSnapshot, SplitSnapshot};
use super::MergeEntity;

/// Reverse a previously-applied cleanup log entry, restoring the
/// pre-mutation state its snapshot recorded.
///
/// Errs [`CleanupApplyError::AlreadyUndone`] if `log_id` was already
/// undone, and [`CleanupApplyError::LogNotFound`] for an unknown id or a
/// `(kind, action)` combination no primitive ever produces.
pub async fn undo(pool: &SqlitePool, log_id: i64) -> Result<(), CleanupApplyError> {
    let row: Option<CleanupLogRow> = sqlx::query_as(
        "SELECT kind, action, snapshot_json, applied_by, undone_at
           FROM cleanup_log WHERE id = ?",
    )
    .bind(log_id)
    .fetch_optional(pool)
    .await?;
    let Some(row) = row else {
        return Err(CleanupApplyError::LogNotFound(log_id));
    };
    if row.undone_at.is_some() {
        return Err(CleanupApplyError::AlreadyUndone);
    }
    let kind = CleanupKind::from_str(&row.kind).ok_or(CleanupApplyError::LogNotFound(log_id))?;
    let action =
        CleanupAction::from_str(&row.action).ok_or(CleanupApplyError::LogNotFound(log_id))?;

    // The rename action routes through `upsert_metadata_overrides`, which
    // owns its own transaction — see `apply::apply_book_title_override`'s
    // doc comment for why this one primitive can't join the shared `tx`
    // every other kind/action combination below undoes inside.
    if kind == CleanupKind::BookTitle && action == CleanupAction::Rename {
        undo_rename(pool, &row.snapshot_json, row.applied_by).await?;
        return mark_undone(pool, log_id).await;
    }

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    match (kind, action) {
        (CleanupKind::Author, CleanupAction::Merge) => {
            undo_merge(&mut tx, MergeEntity::Author, &row.snapshot_json).await?
        }
        (CleanupKind::Series, CleanupAction::Merge) => {
            undo_merge(&mut tx, MergeEntity::Series, &row.snapshot_json).await?
        }
        (CleanupKind::Tag, CleanupAction::Merge) => {
            undo_merge(&mut tx, MergeEntity::Tag, &row.snapshot_json).await?
        }
        (CleanupKind::Tag, CleanupAction::Split) => undo_split(&mut tx, &row.snapshot_json).await?,
        (CleanupKind::Author, CleanupAction::Delete) => {
            undo_delete_author(&mut tx, &row.snapshot_json).await?
        }
        _ => return Err(CleanupApplyError::LogNotFound(log_id)),
    }
    mark_undone_tx(&mut tx, log_id).await?;
    tx.commit().await?;
    Ok(())
}

#[derive(sqlx::FromRow)]
struct CleanupLogRow {
    kind: String,
    action: String,
    snapshot_json: String,
    applied_by: Option<i64>,
    undone_at: Option<i64>,
}

/// Stamp `undone_at`, guarded on `undone_at IS NULL` so two concurrent
/// undo calls can't both report success.
async fn mark_undone(pool: &SqlitePool, log_id: i64) -> Result<(), CleanupApplyError> {
    let result = sqlx::query(
        "UPDATE cleanup_log SET undone_at = strftime('%s','now')
          WHERE id = ? AND undone_at IS NULL",
    )
    .bind(log_id)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(CleanupApplyError::AlreadyUndone);
    }
    Ok(())
}

async fn mark_undone_tx(
    tx: &mut Transaction<'_, Sqlite>,
    log_id: i64,
) -> Result<(), CleanupApplyError> {
    let result = sqlx::query(
        "UPDATE cleanup_log SET undone_at = strftime('%s','now')
          WHERE id = ? AND undone_at IS NULL",
    )
    .bind(log_id)
    .execute(&mut **tx)
    .await?;
    if result.rows_affected() == 0 {
        return Err(CleanupApplyError::AlreadyUndone);
    }
    Ok(())
}

/// Undo `apply_book_title_override`: restore the previous overrides blob,
/// or delete the override row entirely if there was none. Reuses the
/// original apply's `applied_by` to attribute the restore, since `undo`
/// takes no actor of its own — `apply_book_title_override` requires a real
/// `applied_by`, so this is always `Some` for a Rename row.
async fn undo_rename(
    pool: &SqlitePool,
    snapshot_json: &str,
    applied_by: Option<i64>,
) -> Result<(), CleanupApplyError> {
    let snap: RenameSnapshot = serde_json::from_str(snapshot_json)?;
    match snap.previous_overrides {
        Some(json) => {
            let overrides: omnibus_shared::MetadataOverrides = serde_json::from_str(&json)?;
            let actor = applied_by.ok_or(CleanupApplyError::MissingActor)?;
            crate::metadata_overrides::upsert_metadata_overrides(
                pool,
                &snap.book_uuid,
                &overrides,
                snap.previous_has_cover_override,
                actor,
            )
            .await?;
        }
        None => {
            crate::metadata_overrides::delete_metadata_overrides(pool, &snap.book_uuid).await?;
        }
    }
    Ok(())
}

/// Undo `apply_merge_authors`/`_series`/`_tags`: recreate every absorbed
/// row, restore its original links (removing the canonical's merge-created
/// link for any book that didn't already have one), restore the photo/sort
/// state a merge may have touched, and drop the `entity_aliases` row.
async fn undo_merge(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    snapshot_json: &str,
) -> Result<(), CleanupApplyError> {
    let snap: MergeSnapshot = serde_json::from_str(snapshot_json)?;
    let mut touched: HashSet<i64> = snap.canonical_book_ids_before.iter().copied().collect();

    for source in &snap.sources {
        let new_id = recreate_entity(tx, entity, &source.name, source.sort.as_deref()).await?;
        for link in &source.links {
            insert_link(tx, entity, link.book_id, new_id, link.position).await?;
            touched.insert(link.book_id);
            if !snap.canonical_book_ids_before.contains(&link.book_id) {
                delete_link(tx, entity, link.book_id, snap.canonical_id).await?;
            }
        }
        if entity == MergeEntity::Author {
            if let Some(photo) = &source.photo {
                write_photo(tx, new_id, photo).await?;
            }
        }
        delete_entity_alias(tx, entity.kind(), &source.name).await?;
    }

    if snap.canonical_sort_was_backfilled {
        clear_sort(tx, entity, snap.canonical_id).await?;
    }
    if entity == MergeEntity::Author {
        restore_canonical_photo(tx, snap.canonical_id, snap.canonical_photo_before.as_ref())
            .await?;
    }

    for book_id in touched {
        upsert_fts(tx, book_id).await?;
    }
    Ok(())
}

/// Undo `apply_tag_split`: recreate the source tag, restore its links, and
/// remove exactly the per-book atom links the split added (an atom tag
/// that pre-existed independently, or gained other links since, is left
/// alone — only [`crate::taxonomy::delete_orphan_taxonomy`] can still reap
/// it if it's now unreferenced).
async fn undo_split(
    tx: &mut Transaction<'_, Sqlite>,
    snapshot_json: &str,
) -> Result<(), CleanupApplyError> {
    let snap: SplitSnapshot = serde_json::from_str(snapshot_json)?;

    let new_id = recreate_entity(tx, MergeEntity::Tag, &snap.source_name, None).await?;
    for &book_id in &snap.links {
        insert_link(tx, MergeEntity::Tag, book_id, new_id, None).await?;
        for atom in &snap.atoms {
            if let Some(atom_id) = lookup_tag_id(tx, atom).await? {
                delete_link(tx, MergeEntity::Tag, book_id, atom_id).await?;
            }
        }
    }
    delete_orphan_taxonomy(tx).await?;

    for &book_id in &snap.links {
        upsert_fts(tx, book_id).await?;
    }
    Ok(())
}

/// Undo `apply_delete_author`: recreate the row, restore its links and
/// photo, and clear the `ignored_authors` blocklist entry so reindex can
/// see the name again.
async fn undo_delete_author(
    tx: &mut Transaction<'_, Sqlite>,
    snapshot_json: &str,
) -> Result<(), CleanupApplyError> {
    let snap: DeleteAuthorSnapshot = serde_json::from_str(snapshot_json)?;

    let new_id = recreate_entity(tx, MergeEntity::Author, &snap.name, snap.sort.as_deref()).await?;
    for link in &snap.links {
        insert_link(tx, MergeEntity::Author, link.book_id, new_id, link.position).await?;
    }
    if let Some(photo) = &snap.photo {
        write_photo(tx, new_id, photo).await?;
    }
    sqlx::query("DELETE FROM ignored_authors WHERE name = ?")
        .bind(&snap.name)
        .execute(&mut **tx)
        .await?;

    for link in &snap.links {
        upsert_fts(tx, link.book_id).await?;
    }
    Ok(())
}
