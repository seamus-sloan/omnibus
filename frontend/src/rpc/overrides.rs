//! Per-book metadata override save/delete (the "edit this book" surface).

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, MetadataOverrides};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Save metadata overrides for a book. Requires `can_edit` or admin.
/// Returns the merged `EbookMetadata` so the client can update its state
/// without a second round-trip.
#[post("/api/rpc/ebook/overrides", pool: PoolExt, user: AuthUser)]
pub async fn rpc_save_overrides(
    uuid: String,
    overrides: MetadataOverrides,
) -> Result<Option<EbookMetadata>> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    if let Err(msg) = overrides.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    // Route through the db layer's read-merge-write (one BEGIN IMMEDIATE) so
    // concurrent edits to the same book can't interleave and drop each other's
    // changes, and a text-only edit keeps the existing cover flag.
    db::merge_metadata_overrides(&pool.0, &uuid, &overrides, user.id)
        .await
        .map_err(|e| internal_rpc_error("save overrides", e))?;
    Ok(db::get_book_by_uuid(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("get ebook", e))?)
}

/// Delete metadata overrides for a book, reverting to scanned values.
#[post("/api/rpc/ebook/overrides/delete", pool: PoolExt, user: AuthUser)]
pub async fn rpc_delete_overrides(uuid: String) -> Result<Option<EbookMetadata>> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    // Resolve uuid → id once so the `invalidate_thumbs` call (id-keyed by
    // the thumbnail pipeline's file layout) stays accurate.
    let Some(book_id) = db::resolve_book_id_by_uuid(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("resolve book id", e))?
    else {
        return Ok(None);
    };
    db::delete_metadata_overrides(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("delete overrides", e))?;
    // `delete_override_cover` + `invalidate_thumbs` are sync `std::fs`
    // operations; run them on the blocking pool so this server function
    // doesn't pin a tokio worker thread (#106).
    let uuid_for_blocking = uuid.clone();
    tokio::task::spawn_blocking(move || {
        db::delete_override_cover(&uuid_for_blocking);
        db::thumbs::invalidate_thumbs(book_id);
    })
    .await
    .map_err(|e| internal_rpc_error("spawn_blocking(delete_override_cover)", e))?;
    Ok(db::get_book_by_uuid(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("get ebook", e))?)
}
