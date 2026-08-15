//! Transactional apply primitives for the library-cleanup surface: execute
//! an accepted `dedup_suggestions` row (or a direct on-page action, no
//! suggestion involved) against the schema, snapshotting enough state into
//! `cleanup_log` for [`super::undo::undo`] to reverse it exactly.
//!
//! Every primitive but [`apply_book_title_override`] runs in one
//! `BEGIN IMMEDIATE` transaction: snapshot the affected rows, mutate, write
//! the `cleanup_log` row, commit. `apply_book_title_override` is the
//! documented exception — see its doc comment. The raw link-row/photo/alias
//! CRUD both this module and `super::undo` replay lives in
//! [`super::entity_ops`]; this file owns only the primitives' orchestration
//! and the merge-specific photo-priority policy.

use std::collections::HashSet;

use omnibus_shared::{CleanupAction, CleanupKind, MetadataOverrides};
use sqlx::{Sqlite, SqlitePool, Transaction};

use crate::metadata_overrides::{
    get_metadata_overrides, upsert_metadata_overrides, MetadataOverridesError,
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
    #[error("merge source and canonical are the same entity: {0}")]
    CanonicalIsSource(i64),
    #[error("merge requires at least one source entity")]
    EmptySources,
    #[error("tag split requires at least two atoms")]
    TooFewAtoms,
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
/// each source's name in `entity_aliases`, then deletes the source rows.
async fn apply_merge(
    pool: &SqlitePool,
    entity: MergeEntity,
    source_ids: &[i64],
    canonical_id: i64,
    suggestion_id: Option<i64>,
    applied_by: Option<i64>,
) -> Result<i64, CleanupApplyError> {
    if source_ids.is_empty() {
        return Err(CleanupApplyError::EmptySources);
    }
    if source_ids.contains(&canonical_id) {
        return Err(CleanupApplyError::CanonicalIsSource(canonical_id));
    }

    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;

    let (_, canonical_sort_before) = fetch_name_sort(&mut tx, entity, canonical_id)
        .await?
        .ok_or(CleanupApplyError::NotFound(canonical_id))?;
    let canonical_book_ids_before = fetch_linked_book_ids(&mut tx, entity, canonical_id).await?;
    let canonical_photo_before = if entity == MergeEntity::Author {
        load_photo_snapshot(&mut tx, canonical_id).await?
    } else {
        None
    };

    let mut affected: HashSet<i64> = canonical_book_ids_before.iter().copied().collect();
    let mut sources = Vec::with_capacity(source_ids.len());
    for &source_id in source_ids {
        let (name, sort) = fetch_name_sort(&mut tx, entity, source_id)
            .await?
            .ok_or(CleanupApplyError::NotFound(source_id))?;
        let links = fetch_links(&mut tx, entity, source_id).await?;
        affected.extend(links.iter().map(|l| l.book_id));
        let photo = if entity == MergeEntity::Author {
            load_photo_snapshot(&mut tx, source_id).await?
        } else {
            None
        };

        // Snapshot whatever this name already aliased — possibly nothing,
        // possibly an earlier merge's mapping — *before* the write below
        // overwrites it, so undo knows whether to restore or delete.
        let previous_alias = fetch_entity_alias(&mut tx, entity.kind(), &name)
            .await?
            .map(|(alias_canonical_id, created_at)| AliasSnapshot {
                canonical_id: alias_canonical_id,
                created_at,
            });

        move_links(&mut tx, entity, source_id, canonical_id).await?;
        delete_links(&mut tx, entity, source_id).await?;
        write_entity_alias(&mut tx, entity.kind(), &name, canonical_id).await?;

        sources.push(MergedSource {
            id: source_id,
            name,
            sort,
            links,
            photo,
            previous_alias,
        });
    }

    let mut canonical_sort_was_backfilled = false;
    if entity.has_sort() && canonical_sort_before.is_none() {
        if let Some(sort) = sources.iter().find_map(|s| s.sort.clone()) {
            set_sort(&mut tx, entity, canonical_id, &sort).await?;
            canonical_sort_was_backfilled = true;
        }
    }

    if entity == MergeEntity::Author {
        reconcile_author_photos(
            &mut tx,
            canonical_id,
            canonical_photo_before.as_ref(),
            &sources,
        )
        .await?;
    }

    for &source_id in source_ids {
        delete_entity_row(&mut tx, entity, source_id).await?;
    }
    delete_orphan_taxonomy(&mut tx).await?;

    for &book_id in &affected {
        upsert_fts(&mut tx, book_id).await?;
    }

    let snapshot = MergeSnapshot {
        canonical_id,
        canonical_sort_before,
        canonical_sort_was_backfilled,
        canonical_photo_before,
        canonical_book_ids_before,
        sources,
    };
    let log_id = write_cleanup_log(
        &mut tx,
        suggestion_id,
        entity.kind(),
        CleanupAction::Merge,
        &snapshot,
        applied_by,
    )
    .await?;

    tx.commit().await?;
    Ok(log_id)
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
        return Err(CleanupApplyError::TooFewAtoms);
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
/// [`upsert_metadata_overrides`] door — never touching `books.title`
/// directly, so the rename composes with every other override field and
/// reverts cleanly.
///
/// Unlike the other primitives, this is **not** a single all-or-nothing
/// transaction: `upsert_metadata_overrides` owns its own `BEGIN IMMEDIATE`
/// transaction and can't be composed into ours. The pre-rename overrides
/// blob is still read strictly before the mutating call (which is what
/// undo needs), and the `cleanup_log` row is written only *after* the
/// mutation commits — so a crash between the read and the mutate leaves no
/// log row (nothing to undo, matching reality), and the only residual gap
/// is a crash between a successful mutate and the log INSERT, which loses
/// undo-ability for that one rename without corrupting any state.
///
/// Takes a required `applied_by` (unlike the other primitives'
/// `Option<i64>`) because `upsert_metadata_overrides` requires a real user
/// id to attribute the write to, and [`super::undo::undo`] reuses that same
/// id to attribute the restore.
pub async fn apply_book_title_override(
    pool: &SqlitePool,
    book_uuid: &str,
    proposed_title: &str,
    suggestion_id: Option<i64>,
    applied_by: i64,
) -> Result<i64, CleanupApplyError> {
    let (previous_overrides, previous_has_cover_override) =
        match get_metadata_overrides(pool, book_uuid).await? {
            Some((ov, has_cover)) => (Some(serde_json::to_string(&ov)?), has_cover),
            None => (None, false),
        };

    let mut new_overrides: MetadataOverrides = match &previous_overrides {
        Some(json) => serde_json::from_str(json)?,
        None => MetadataOverrides::default(),
    };
    new_overrides.title = Some(proposed_title.to_string());

    upsert_metadata_overrides(
        pool,
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
    let log_id: i64 = sqlx::query_scalar(
        "INSERT INTO cleanup_log (suggestion_id, kind, action, snapshot_json, applied_by)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(suggestion_id)
    .bind(CleanupKind::BookTitle.as_str())
    .bind(CleanupAction::Rename.as_str())
    .bind(serde_json::to_string(&snapshot)?)
    .bind(applied_by)
    .fetch_one(pool)
    .await?;
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
