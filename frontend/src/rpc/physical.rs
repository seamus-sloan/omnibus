//! Physical-collection server functions for book detail: a book's library-wide
//! copies, the caller's wishlist entry, and fileless-book removal. Mobile uses
//! the analogous REST routes in `server::backend::physical`.

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::physical::{PhysicalCopy, WishlistEntry};

#[cfg(feature = "server")]
use omnibus_db as db;
// Only the server-side body names the source; the client half never sees it.
#[cfg(feature = "server")]
use omnibus_shared::physical::WishlistSource;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Map a data-layer error onto the wire. Only the cases book detail can act on
/// get a typed message; the rest collapse to an opaque internal error.
#[cfg(feature = "server")]
fn map_physical_error(context: &'static str, e: db::PhysicalError) -> ServerFnError {
    match e {
        db::PhysicalError::BookNotFound => ServerFnError::new("book not found"),
        db::PhysicalError::CopyNotFound => ServerFnError::new("physical copy not found"),
        db::PhysicalError::BookHasFiles => {
            ServerFnError::new("book still has files; remove them first")
        }
        other => internal_rpc_error(context, other),
    }
}

/// Reject a caller without `can_edit`. Copies are library-wide, so an edit
/// changes what every user sees — same gate as the metadata-override writes.
#[cfg(feature = "server")]
fn require_edit(user: &AuthUser) -> Result<(), ServerFnError> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("edit permission required"));
    }
    Ok(())
}

/// A book's physical copies, oldest check-in first. An unknown uuid has none.
// `_user` is bound purely to gate the call on a session; the read itself is
// library-wide, not per-user.
#[post("/api/rpc/physical/copies", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_list_physical_copies(uuid: String) -> Result<Vec<PhysicalCopy>> {
    Ok(db::list_physical_copies(&pool.0, &uuid)
        .await
        .map_err(|e| map_physical_error("list physical copies", e))?)
}

/// Replace a copy's free-text note, returning the updated copy. A blank note
/// clears it.
#[post("/api/rpc/physical/copies/note", pool: PoolExt, user: AuthUser)]
pub async fn rpc_update_physical_copy_note(
    copy_id: i64,
    note: Option<String>,
) -> Result<PhysicalCopy> {
    require_edit(&user)?;
    Ok(
        db::update_physical_copy_note(&pool.0, copy_id, note.as_deref())
            .await
            .map_err(|e| map_physical_error("update physical copy note", e))?,
    )
}

/// Delete one physical copy ("I sold it").
#[post("/api/rpc/physical/copies/delete", pool: PoolExt, user: AuthUser)]
pub async fn rpc_delete_physical_copy(copy_id: i64) -> Result<()> {
    require_edit(&user)?;
    Ok(db::delete_physical_copy(&pool.0, copy_id)
        .await
        .map_err(|e| map_physical_error("delete physical copy", e))?)
}

/// The caller's wishlist entry for a book, or `None` when not wishlisted.
#[post("/api/rpc/physical/wishlist/get", pool: PoolExt, user: AuthUser)]
pub async fn rpc_get_wishlist_entry(uuid: String) -> Result<Option<WishlistEntry>> {
    Ok(db::get_wishlist_entry(&pool.0, user.id, &uuid)
        .await
        .map_err(|e| map_physical_error("get wishlist entry", e))?)
}

/// Add a book to the caller's wishlist from its detail page. Idempotent — a
/// second add returns the existing entry unchanged.
#[post("/api/rpc/physical/wishlist/add", pool: PoolExt, user: AuthUser)]
pub async fn rpc_add_wishlist_entry(uuid: String) -> Result<WishlistEntry> {
    Ok(
        db::add_wishlist_entry(&pool.0, user.id, &uuid, WishlistSource::Detail)
            .await
            .map_err(|e| map_physical_error("add wishlist entry", e))?,
    )
}

/// Remove a book from the caller's wishlist. A no-op when absent.
#[post("/api/rpc/physical/wishlist/remove", pool: PoolExt, user: AuthUser)]
pub async fn rpc_remove_wishlist_entry(uuid: String) -> Result<()> {
    Ok(db::remove_wishlist_entry(&pool.0, user.id, &uuid)
        .await
        .map_err(|e| map_physical_error("remove wishlist entry", e))?)
}

/// Delete a fileless book outright — the "remove it entirely" branch of the
/// last-copy prompt. Errors when the book still has digital files.
#[post("/api/rpc/physical/book/delete", pool: PoolExt, user: AuthUser)]
pub async fn rpc_delete_fileless_book(uuid: String) -> Result<()> {
    require_edit(&user)?;
    Ok(db::delete_fileless_book(&pool.0, &uuid)
        .await
        .map_err(|e| map_physical_error("delete fileless book", e))?)
}
