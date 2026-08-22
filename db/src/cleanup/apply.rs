//! Transactional apply primitives for the library-cleanup surface: execute
//! an accepted `dedup_suggestions` row (or a direct on-page action) against
//! the schema, snapshotting enough state into `cleanup_log` for
//! [`super::undo::undo`] to reverse it exactly. Every primitive runs in one
//! `BEGIN IMMEDIATE` transaction.

use std::collections::HashSet;

use omnibus_shared::{CleanupAction, CleanupKind, MetadataOverrides};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::metadata_overrides::{
    get_metadata_overrides_exec, rebuild_fts_for_book, upsert_one_in_tx, MetadataOverridesError,
};
use crate::sync::upsert_fts;
use crate::taxonomy::{delete_orphan_taxonomy, resolve_or_insert_tag};

use super::entity_ops::{
    delete_entity_row, delete_links, fetch_entity_alias, fetch_linked_book_ids, fetch_links,
    fetch_name_sort, load_photo_snapshot, move_links, set_sort, write_cleanup_log,
    write_entity_alias, write_photo,
};
use super::snapshot::{
    AliasSnapshot, DeleteAuthorSnapshot, MergeSnapshot, MergedSource, PhotoSnapshot,
    RenameSnapshot, SplitSnapshot,
};
use super::MergeEntity;

#[cfg(test)]
mod tests;

/// Errors from the cleanup apply/undo layer.
#[derive(Debug, thiserror::Error)]
pub enum CleanupApplyError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    #[error("cleanup snapshot encode/decode failed: {0}")]
    Snapshot(#[from] serde_json::Error),
    #[error(transparent)]
    Overrides(#[from] MetadataOverridesError),
    #[error("entity not found: {0}")]
    NotFound(i64),
    /// A caller's arguments fail a primitive's own precondition. Coarse on
    /// purpose: no caller branches on *which* precondition, every one of them
    /// is rendered as this sentence.
    #[error("{0}")]
    InvalidRequest(String),
    #[error("cleanup log entry not found: {0}")]
    LogNotFound(i64),
    #[error("cleanup was already undone")]
    AlreadyUndone,
    #[error("undo has no applying user recorded to attribute a title restore to")]
    MissingActor,
}

// ---------------------------------------------------------------------------
// Merge (author / series / tag)
// ---------------------------------------------------------------------------

/// Merge `source_ids` into `canonical_id`, moving every affected book's
/// author link, reconciling `author_photos` by priority (manual >
/// openlibrary > letter), and backfilling a NULL `sort` on the survivor.
pub async fn apply_merge_authors(
    pool: &SqlitePool,
    source_ids: &[i64],
    canonical_id: i64,
    suggestion_id: Option<i64>,
    applied_by: Option<i64>,
) -> Result<i64, CleanupApplyError> {
    apply_merge(
        pool,
        MergeEntity::Author,
        source_ids,
        canonical_id,
        suggestion_id,
        applied_by,
    )
    .await
}

/// Merge `source_ids` into `canonical_id`, moving every affected book's
/// series link and backfilling a NULL `sort` on the survivor.
pub async fn apply_merge_series(
    pool: &SqlitePool,
    source_ids: &[i64],
    canonical_id: i64,
    suggestion_id: Option<i64>,
    applied_by: Option<i64>,
) -> Result<i64, CleanupApplyError> {
    apply_merge(
        pool,
        MergeEntity::Series,
        source_ids,
        canonical_id,
        suggestion_id,
        applied_by,
    )
    .await
}

/// Merge `source_ids` into `canonical_id`, moving every affected book's tag
/// link.
pub async fn apply_merge_tags(
    pool: &SqlitePool,
    source_ids: &[i64],
    canonical_id: i64,
    suggestion_id: Option<i64>,
    applied_by: Option<i64>,
) -> Result<i64, CleanupApplyError> {
    apply_merge(
        pool,
        MergeEntity::Tag,
        source_ids,
        canonical_id,
        suggestion_id,
        applied_by,
    )
    .await
}

/// Shared merge body for authors/series/tags. Snapshots every source's
/// name/sort/links(/photo), moves links onto the canonical id with
/// `INSERT OR IGNORE` (survives a book already linked to both), records
/// each source's name in `entity_aliases`, then deletes the source rows —
/// staged across [`snapshot_canonical`], [`move_source_links`],
/// [`reconcile_photos_and_sort`], [`delete_sources_and_reap_taxonomy`], and
/// [`rederive_fts_and_log`], all inside the one transaction this function
/// owns.
async fn apply_merge(
    pool: &SqlitePool,
    entity: MergeEntity,
    source_ids: &[i64],
    canonical_id: i64,
    suggestion_id: Option<i64>,
    applied_by: Option<i64>,
) -> Result<i64, CleanupApplyError> {
    if source_ids.is_empty() {
        return Err(CleanupApplyError::InvalidRequest(
            "merge requires at least one source entity".to_string(),
        ));
    }
    if source_ids.contains(&canonical_id) {
        return Err(CleanupApplyError::InvalidRequest(format!(
            "merge source and canonical are the same entity: {canonical_id}"
        )));
    }

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let (canonical_sort_before, canonical_book_ids_before, canonical_photo_before) =
        snapshot_canonical(&mut tx, entity, canonical_id).await?;

    let mut affected: HashSet<i64> = canonical_book_ids_before.iter().copied().collect();
    let sources =
        move_source_links(&mut tx, entity, source_ids, canonical_id, &mut affected).await?;

    let canonical_sort_was_backfilled = reconcile_photos_and_sort(
        &mut tx,
        entity,
        canonical_id,
        canonical_sort_before.as_deref(),
        canonical_photo_before.as_ref(),
        &sources,
    )
    .await?;

    delete_sources_and_reap_taxonomy(&mut tx, entity, source_ids).await?;

    let snapshot = MergeSnapshot {
        canonical_id,
        canonical_sort_before,
        canonical_sort_was_backfilled,
        canonical_photo_before,
        canonical_book_ids_before,
        sources,
    };
    let log_id = rederive_fts_and_log(
        &mut tx,
        &affected,
        suggestion_id,
        entity.kind(),
        &snapshot,
        applied_by,
    )
    .await?;

    tx.commit().await?;
    Ok(log_id)
}

/// Read the canonical entity's pre-merge sort, linked book ids, and (for
/// authors) photo — the "before" state
/// [`undo_merge`](super::undo::undo_merge) needs to restore it exactly.
async fn snapshot_canonical(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    canonical_id: i64,
) -> Result<(Option<String>, Vec<i64>, Option<PhotoSnapshot>), CleanupApplyError> {
    let (_, canonical_sort_before) = fetch_name_sort(tx, entity, canonical_id)
        .await?
        .ok_or(CleanupApplyError::NotFound(canonical_id))?;
    let canonical_book_ids_before = fetch_linked_book_ids(tx, entity, canonical_id).await?;
    let canonical_photo_before = if entity == MergeEntity::Author {
        load_photo_snapshot(tx, canonical_id).await?
    } else {
        None
    };
    Ok((
        canonical_sort_before,
        canonical_book_ids_before,
        canonical_photo_before,
    ))
}

/// For each source id: snapshot its name/sort/links(/photo) and whatever it
/// already aliased, move its links onto `canonical_id`, drop its own links,
/// and record its name in `entity_aliases`. Every touched book id — from
/// both the canonical's pre-merge links (via the caller-seeded `affected`)
/// and each source's — accumulates into `affected` for the FTS rebuild
/// pass. Returns one [`MergedSource`] per source id, in `source_ids` order.
async fn move_source_links(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    source_ids: &[i64],
    canonical_id: i64,
    affected: &mut HashSet<i64>,
) -> Result<Vec<MergedSource>, CleanupApplyError> {
    let mut sources = Vec::with_capacity(source_ids.len());
    for &source_id in source_ids {
        let (name, sort) = fetch_name_sort(tx, entity, source_id)
            .await?
            .ok_or(CleanupApplyError::NotFound(source_id))?;
        let links = fetch_links(tx, entity, source_id).await?;
        affected.extend(links.iter().map(|l| l.book_id));
        let photo = if entity == MergeEntity::Author {
            load_photo_snapshot(tx, source_id).await?
        } else {
            None
        };

        // Snapshot whatever this name already aliased — possibly nothing,
        // possibly an earlier merge's mapping — *before* the write below
        // overwrites it, so undo knows whether to restore or delete.
        let previous_alias = fetch_entity_alias(tx, entity.kind(), &name).await?.map(
            |(alias_canonical_id, created_at)| AliasSnapshot {
                canonical_id: alias_canonical_id,
                created_at,
            },
        );

        move_links(tx, entity, source_id, canonical_id).await?;
        delete_links(tx, entity, source_id).await?;
        write_entity_alias(tx, entity.kind(), &name, canonical_id).await?;

        sources.push(MergedSource {
            id: source_id,
            name,
            sort,
            links,
            photo,
            previous_alias,
        });
    }
    Ok(sources)
}

/// Backfill a NULL canonical sort from the first source that has one, and
/// (for authors) reconcile the canonical's photo against every merged
/// source's by priority. Returns whether the sort was backfilled, for the
/// snapshot's `canonical_sort_was_backfilled` field.
async fn reconcile_photos_and_sort(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    canonical_id: i64,
    canonical_sort_before: Option<&str>,
    canonical_photo_before: Option<&PhotoSnapshot>,
    sources: &[MergedSource],
) -> Result<bool, sqlx::Error> {
    let mut canonical_sort_was_backfilled = false;
    if entity.has_sort() && canonical_sort_before.is_none() {
        if let Some(sort) = sources.iter().find_map(|s| s.sort.clone()) {
            set_sort(tx, entity, canonical_id, &sort).await?;
            canonical_sort_was_backfilled = true;
        }
    }
    if entity == MergeEntity::Author {
        reconcile_author_photos(tx, canonical_id, canonical_photo_before, sources).await?;
    }
    Ok(canonical_sort_was_backfilled)
}

/// Delete every source entity row now that their links have moved, then
/// reap any taxonomy row (tag/series/author) left with zero links as a
/// result.
async fn delete_sources_and_reap_taxonomy(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    source_ids: &[i64],
) -> Result<(), sqlx::Error> {
    for &source_id in source_ids {
        delete_entity_row(tx, entity, source_id).await?;
    }
    delete_orphan_taxonomy(tx).await?;
    Ok(())
}

/// Rebuild `books_fts` for every book the merge touched, then write the
/// `cleanup_log` row recording `snapshot` so [`super::undo::undo`] can
/// reverse it. Returns the new log row's id.
async fn rederive_fts_and_log(
    tx: &mut Transaction<'_, Sqlite>,
    affected: &HashSet<i64>,
    suggestion_id: Option<i64>,
    kind: CleanupKind,
    snapshot: &MergeSnapshot,
    applied_by: Option<i64>,
) -> Result<i64, CleanupApplyError> {
    for &book_id in affected {
        upsert_fts(tx, book_id).await?;
    }
    write_cleanup_log(
        tx,
        suggestion_id,
        kind,
        CleanupAction::Merge,
        snapshot,
        applied_by,
    )
    .await
}

/// Priority rank for [`reconcile_author_photos`]: lower wins. No photo at
/// all ranks lowest, so any real photo beats having none.
fn photo_rank(p: Option<&PhotoSnapshot>) -> u8 {
    match p.map(|p| p.source.as_str()) {
        Some("manual") => 0,
        Some("openlibrary") => 1,
        Some("letter") => 2,
        _ => 3,
    }
}

/// Reconcile the canonical author's photo against every merged source's,
/// by priority manual > openlibrary > letter > none. Only writes when a
/// source's photo strictly outranks the canonical's own.
async fn reconcile_author_photos(
    tx: &mut Transaction<'_, Sqlite>,
    canonical_id: i64,
    canonical_before: Option<&PhotoSnapshot>,
    sources: &[MergedSource],
) -> Result<(), sqlx::Error> {
    let mut best_rank = photo_rank(canonical_before);
    let mut best: Option<&PhotoSnapshot> = None;
    for s in sources {
        let rank = photo_rank(s.photo.as_ref());
        if rank < best_rank {
            best_rank = rank;
            best = s.photo.as_ref();
        }
    }
    if let Some(p) = best {
        write_photo(tx, canonical_id, p).await?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Tag split
// ---------------------------------------------------------------------------

/// Replace tag `source_id` with `atoms`: resolve-or-insert each atom,
/// link it to every book the source tag was linked to, drop the source's
/// links, then delete the source tag row. Each atom's linking is one
/// set-based `INSERT ... SELECT` (via [`move_links`]) reading the source's
/// current `books_tags_link` rows, not a per-book round trip — a popular
/// tag split into a handful of atoms stays O(atoms) queries rather than
/// O(atoms × books).
pub async fn apply_tag_split(
    pool: &SqlitePool,
    source_id: i64,
    delimiter: &str,
    atoms: &[String],
    suggestion_id: Option<i64>,
    applied_by: Option<i64>,
) -> Result<i64, CleanupApplyError> {
    if atoms.len() < 2 {
        return Err(CleanupApplyError::InvalidRequest(
            "tag split requires at least two atoms".to_string(),
        ));
    }

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let (source_name, _) = fetch_name_sort(&mut tx, MergeEntity::Tag, source_id)
        .await?
        .ok_or(CleanupApplyError::NotFound(source_id))?;
    let book_ids = fetch_linked_book_ids(&mut tx, MergeEntity::Tag, source_id).await?;

    for atom in atoms {
        let atom_id = resolve_or_insert_tag(&mut tx, atom).await?;
        // `book_ids` above is exactly the set `books_tags_link WHERE tag =
        // source_id` currently holds (the source's links aren't dropped
        // until after this loop), so moving them onto each atom is the same
        // set-based copy `move_links` already does for a merge.
        move_links(&mut tx, MergeEntity::Tag, source_id, atom_id).await?;
    }

    delete_links(&mut tx, MergeEntity::Tag, source_id).await?;
    delete_entity_row(&mut tx, MergeEntity::Tag, source_id).await?;
    delete_orphan_taxonomy(&mut tx).await?;

    for &book_id in &book_ids {
        upsert_fts(&mut tx, book_id).await?;
    }

    let snapshot = SplitSnapshot {
        source_name,
        delimiter: delimiter.to_string(),
        atoms: atoms.to_vec(),
        links: book_ids,
    };
    let log_id = write_cleanup_log(
        &mut tx,
        suggestion_id,
        CleanupKind::Tag,
        CleanupAction::Split,
        &snapshot,
        applied_by,
    )
    .await?;

    tx.commit().await?;
    Ok(log_id)
}

// ---------------------------------------------------------------------------
// Book title rename
// ---------------------------------------------------------------------------

/// Adopt `proposed_title` for `book_uuid` by routing through the existing
/// metadata-overrides door — never touching `books.title` directly, so the
/// rename composes with every other override field and reverts cleanly.
///
/// Like the other primitives, this runs in one `BEGIN IMMEDIATE`
/// transaction: the previous-overrides snapshot is read through that same
/// transaction (not the pool), then it composes [`upsert_one_in_tx`] (the
/// tx-scoped body behind the pool-level
/// [`crate::metadata_overrides::upsert_metadata_overrides`]) with the
/// `cleanup_log` INSERT — so the snapshot read, the override write, and the
/// log row all agree with each other and land together or not at all. The
/// `books_fts` rebuild still runs best-effort after commit, matching every
/// other override-write caller: a stale FTS row is recoverable and far less
/// consequential than a stale series/tag link.
///
/// Takes a required `applied_by` (unlike the other primitives'
/// `Option<i64>`) because the metadata-overrides write requires a real user
/// id to attribute the write to, and [`super::undo::undo`] reuses that same
/// id to attribute the restore.
pub async fn apply_book_title_override(
    pool: &SqlitePool,
    book_uuid: &str,
    proposed_title: &str,
    suggestion_id: Option<i64>,
    applied_by: i64,
) -> Result<i64, CleanupApplyError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    // Read the snapshot through this same transaction, not the pool, so a
    // concurrent writer can't commit an override change between the read
    // and this transaction's start — which would log a stale snapshot and
    // make undo restore the wrong prior state.
    let (previous_overrides, previous_has_cover_override) =
        match get_metadata_overrides_exec(&mut *tx, book_uuid).await? {
            Some((ov, has_cover)) => (Some(serde_json::to_string(&ov)?), has_cover),
            None => (None, false),
        };

    let mut new_overrides: MetadataOverrides = match &previous_overrides {
        Some(json) => serde_json::from_str(json)?,
        None => MetadataOverrides::default(),
    };
    new_overrides.title = Some(proposed_title.to_string());

    upsert_one_in_tx(
        &mut tx,
        book_uuid,
        &new_overrides,
        previous_has_cover_override,
        applied_by,
    )
    .await?;

    let snapshot = RenameSnapshot {
        book_uuid: book_uuid.to_string(),
        previous_overrides,
        previous_has_cover_override,
    };
    let log_id = write_cleanup_log(
        &mut tx,
        suggestion_id,
        CleanupKind::BookTitle,
        CleanupAction::Rename,
        &snapshot,
        Some(applied_by),
    )
    .await?;

    tx.commit().await?;

    if let Err(e) = rebuild_fts_for_book(pool, book_uuid).await {
        tracing::warn!(book_uuid, error = %e, "books_fts rebuild after title override apply failed");
    }

    Ok(log_id)
}

// ---------------------------------------------------------------------------
// Author delete (junk-author rows)
// ---------------------------------------------------------------------------

/// Delete an author row outright: unlink every book, blocklist the name in
/// `ignored_authors` so reindex doesn't silently recreate it (mirrors
/// [`crate::author_photos_data::delete_author`], but additionally
/// snapshots into `cleanup_log` so it can be undone).
pub async fn apply_delete_author(
    pool: &SqlitePool,
    author_id: i64,
    suggestion_id: Option<i64>,
    applied_by: Option<i64>,
) -> Result<i64, CleanupApplyError> {
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let (name, sort) = fetch_name_sort(&mut tx, MergeEntity::Author, author_id)
        .await?
        .ok_or(CleanupApplyError::NotFound(author_id))?;
    let links = fetch_links(&mut tx, MergeEntity::Author, author_id).await?;
    let photo = load_photo_snapshot(&mut tx, author_id).await?;

    delete_links(&mut tx, MergeEntity::Author, author_id).await?;
    delete_entity_row(&mut tx, MergeEntity::Author, author_id).await?;
    sqlx::query("INSERT OR IGNORE INTO ignored_authors(name) VALUES (?)")
        .bind(&name)
        .execute(&mut *tx)
        .await?;
    delete_orphan_taxonomy(&mut tx).await?;

    for link in &links {
        upsert_fts(&mut tx, link.book_id).await?;
    }

    let snapshot = DeleteAuthorSnapshot {
        name,
        sort,
        links,
        photo,
    };
    let log_id = write_cleanup_log(
        &mut tx,
        suggestion_id,
        CleanupKind::Author,
        CleanupAction::Delete,
        &snapshot,
        applied_by,
    )
    .await?;

    tx.commit().await?;
    Ok(log_id)
}
