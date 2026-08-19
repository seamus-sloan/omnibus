//! Reverses an applied cleanup by replaying the `cleanup_log.snapshot_json`
//! a `super::apply` primitive wrote. Mirrors `apply` action-for-action —
//! any drift between the two shows up as a failing round-trip test rather
//! than a silent partial undo.

use std::collections::HashSet;

use omnibus_shared::{CleanupAction, CleanupKind};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::sync::upsert_fts_batch;
use crate::taxonomy::delete_orphan_taxonomy;

use super::apply::CleanupApplyError;
use super::entity_ops::{
    clear_sort, delete_entity_alias, delete_link, delete_links_bulk, insert_link,
    insert_links_bulk, lookup_tag_ids, recreate_entity, restore_canonical_photo,
    restore_entity_alias, write_photo,
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

    // The row-level check above is only an optimistic fast path (it reads
    // outside any transaction) — the real guard is `mark_undone_tx` below,
    // run *first* inside the transaction, before any replay touches a
    // table. Two concurrent `undo(log_id)` calls both begin
    // `BEGIN IMMEDIATE`; SQLite's single-writer lock admits only one at a
    // time, and whichever runs second finds `undone_at` already set, so its
    // claim affects zero rows and it errors out (rolling back its empty
    // transaction) *before* replaying a single mutation. Claim-then-mutate,
    // never the reverse, is what makes "already undone" mean "nothing this
    // call did landed" rather than "something landed, but here's an error
    // anyway" — this is why the `BookTitle`/`Rename` path below claims and
    // replays inside this same transaction too, rather than against its own
    // separate one the way `apply::apply_book_title_override` must (see
    // that primitive's doc comment for why *applying* a rename can't join a
    // shared transaction — undoing one has no such constraint, since the
    // restore is a plain `metadata_overrides` write with no second owned
    // transaction of its own once it's expressed as `upsert_one_in_tx`/
    // `delete_one_in_tx`).
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    mark_undone_tx(&mut tx, log_id).await?;
    let mut fts_rebuild_uuid: Option<String> = None;
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
        (CleanupKind::BookTitle, CleanupAction::Rename) => {
            fts_rebuild_uuid = undo_rename(&mut tx, &row.snapshot_json, row.applied_by).await?;
        }
        _ => return Err(CleanupApplyError::LogNotFound(log_id)),
    }
    tx.commit().await?;

    // Best-effort, after commit — matches `upsert_metadata_overrides`'s own
    // post-commit FTS rebuild exactly (see `undo_rename`): a stale FTS row
    // self-heals on the next reindex or save and isn't worth failing an
    // otherwise-successful undo over.
    if let Some(book_uuid) = fts_rebuild_uuid {
        if let Err(e) = crate::metadata_overrides::rebuild_fts_for_book(pool, &book_uuid).await {
            tracing::warn!(book_uuid, error = %e, "books_fts rebuild after title-rename undo failed");
        }
    }
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

/// Claim the log row by stamping `undone_at`, guarded on `undone_at IS
/// NULL` so of two racing transactions only one can affect a row — the
/// other's `rows_affected() == 0` and it errors here, before either has run
/// a single replay step (see the ordering note in [`undo`]).
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
/// or delete the override row entirely if there was none — joining the
/// caller's transaction via `upsert_one_in_tx`/`delete_one_in_tx` so the
/// claim in [`undo`] and this restore commit or roll back together. Returns
/// `Some(book_uuid)` when the restore path needs the post-commit FTS
/// rebuild `upsert_metadata_overrides` would normally trigger itself — the
/// delete path already restores canonical FTS inside its own tx-scoped
/// call, matching `delete_metadata_overrides`.
///
/// Reuses the original apply's `applied_by` to attribute the restore, since
/// `undo` takes no actor of its own — `apply_book_title_override` requires
/// a real `applied_by`, so this is always `Some` for a Rename row.
async fn undo_rename(
    tx: &mut Transaction<'_, Sqlite>,
    snapshot_json: &str,
    applied_by: Option<i64>,
) -> Result<Option<String>, CleanupApplyError> {
    let snap: RenameSnapshot = serde_json::from_str(snapshot_json)?;
    match snap.previous_overrides {
        Some(json) => {
            let overrides: omnibus_shared::MetadataOverrides = serde_json::from_str(&json)?;
            let actor = applied_by.ok_or(CleanupApplyError::MissingActor)?;
            crate::metadata_overrides::upsert_one_in_tx(
                tx,
                &snap.book_uuid,
                &overrides,
                snap.previous_has_cover_override,
                actor,
            )
            .await?;
            Ok(Some(snap.book_uuid))
        }
        None => {
            crate::metadata_overrides::delete_one_in_tx(tx, &snap.book_uuid).await?;
            Ok(None)
        }
    }
}

/// Undo `apply_merge_authors`/`_series`/`_tags`: recreate every absorbed
/// row, restore its original links (removing the canonical's merge-created
/// link for any book that didn't already have one), restore the photo/sort
/// state a merge may have touched, and restore the `entity_aliases` row to
/// whatever it held before this merge — or delete it, if nothing did.
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
        // A name with no prior alias gets its row deleted outright; a name
        // that already aliased something else (an earlier merge) gets that
        // mapping restored rather than erased.
        match &source.previous_alias {
            Some(prev) => {
                restore_entity_alias(
                    tx,
                    entity.kind(),
                    &source.name,
                    prev.canonical_id,
                    prev.created_at,
                )
                .await?
            }
            None => delete_entity_alias(tx, entity.kind(), &source.name).await?,
        }
    }

    if snap.canonical_sort_was_backfilled {
        clear_sort(tx, entity, snap.canonical_id).await?;
    }
    if entity == MergeEntity::Author {
        restore_canonical_photo(tx, snap.canonical_id, snap.canonical_photo_before.as_ref())
            .await?;
    }

    let touched: Vec<i64> = touched.into_iter().collect();
    upsert_fts_batch(tx, &touched).await?;
    Ok(())
}

/// Undo `apply_tag_split`: recreate the source tag, restore its links, and
/// remove exactly the per-book atom links the split added (an atom tag
/// that pre-existed independently, or gained other links since, is left
/// alone — only [`crate::taxonomy::delete_orphan_taxonomy`] can still reap
/// it if it's now unreferenced).
///
/// Batched, not one query per `(book, atom)` pair: [`lookup_tag_ids`]
/// resolves every atom name to an id in one pass, [`insert_links_bulk`]
/// restores every book's link to the recreated source tag in one pass, and
/// [`delete_links_bulk`] removes every atom link the split added in one
/// pass — O(books + atoms) round trips instead of the O(books × atoms) the
/// naive per-pair replay ran (mirrors `apply_tag_split`'s own set-based
/// shape in `apply.rs`).
async fn undo_split(
    tx: &mut Transaction<'_, Sqlite>,
    snapshot_json: &str,
) -> Result<(), CleanupApplyError> {
    let snap: SplitSnapshot = serde_json::from_str(snapshot_json)?;

    let new_id = recreate_entity(tx, MergeEntity::Tag, &snap.source_name, None).await?;
    insert_links_bulk(tx, MergeEntity::Tag, new_id, &snap.links).await?;

    let atom_ids = lookup_tag_ids(tx, &snap.atoms).await?;
    delete_links_bulk(tx, MergeEntity::Tag, &snap.links, &atom_ids).await?;

    delete_orphan_taxonomy(tx).await?;

    upsert_fts_batch(tx, &snap.links).await?;
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

    let book_ids: Vec<i64> = snap.links.iter().map(|link| link.book_id).collect();
    upsert_fts_batch(tx, &book_ids).await?;
    Ok(())
}
