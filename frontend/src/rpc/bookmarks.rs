//! Bookmark CRUD shared by the reader (CFI positions) and the audiobook
//! player (second positions).

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::{Bookmark, CreateBookmark, UpdateBookmark};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_error, AuthUser, PoolExt};

/// Create a bookmark on a book. Mobile uses the analogous REST route in
/// `server::backend::bookmarks`; the rest of this family follows the same
/// web-vs-mobile split. One model serves the audiobook player (position =
/// seconds) and the reader (position = EPUB CFI).
#[post("/api/rpc/bookmarks/create", pool: PoolExt, user: AuthUser)]
pub async fn rpc_create_bookmark(input: CreateBookmark) -> Result<Bookmark> {
    if let Err(msg) = input.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    match db::bookmarks::create_bookmark(&pool.0, user.id, &input).await {
        Ok(b) => Ok(b),
        Err(db::bookmarks::BookmarkError::BookNotFound) => {
            Err(ServerFnError::new("book not found").into())
        }
        // NotFound on create means the just-inserted row couldn't be re-read —
        // a server invariant break, not a missing client resource. Log it and
        // surface a generic internal error rather than a misleading 404.
        Err(e @ db::bookmarks::BookmarkError::NotFound) => {
            tracing::error!(error = %e, "rpc_create_bookmark: inserted row could not be re-read");
            Err(ServerFnError::new("internal server error").into())
        }
        Err(db::bookmarks::BookmarkError::Sqlx(e)) => Err(internal_error("create", e).into()),
    }
}

/// List all bookmarks for the given book uuid, scoped to the current user and
/// ordered by creation time. Returns an empty list — not an error — when the
/// uuid is unknown or has no bookmarks yet.
#[post("/api/rpc/bookmarks/list", pool: PoolExt, user: AuthUser)]
pub async fn rpc_list_bookmarks(book_uuid: String) -> Result<Vec<Bookmark>> {
    match db::bookmarks::list_bookmarks(&pool.0, user.id, &book_uuid).await {
        Ok(list) => Ok(list),
        Err(db::bookmarks::BookmarkError::Sqlx(e)) => Err(internal_error("list", e).into()),
        Err(e) => Err(internal_error("list", e).into()),
    }
}

/// Set or clear the title/note on a bookmark owned by the current user.
/// Validates `body` against `UpdateBookmark::validate()` and errors with
/// `"bookmark not found"` when the id does not exist or belongs to another
/// user (the two cases are deliberately indistinguishable).
#[post("/api/rpc/bookmarks/update", pool: PoolExt, user: AuthUser)]
pub async fn rpc_update_bookmark(id: i64, body: UpdateBookmark) -> Result<()> {
    if let Err(msg) = body.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    match db::bookmarks::update_bookmark(&pool.0, user.id, id, body.title.as_deref()).await {
        Ok(()) => Ok(()),
        Err(db::bookmarks::BookmarkError::NotFound) => {
            Err(ServerFnError::new("bookmark not found").into())
        }
        Err(db::bookmarks::BookmarkError::Sqlx(e)) => Err(internal_error("update", e).into()),
        Err(e) => Err(internal_error("update", e).into()),
    }
}

/// Delete a bookmark owned by the current user. Errors with
/// `"bookmark not found"` when the id does not exist or belongs to another
/// user (the two cases are deliberately indistinguishable).
#[post("/api/rpc/bookmarks/delete", pool: PoolExt, user: AuthUser)]
pub async fn rpc_delete_bookmark(id: i64) -> Result<()> {
    match db::bookmarks::delete_bookmark(&pool.0, user.id, id).await {
        Ok(()) => Ok(()),
        Err(db::bookmarks::BookmarkError::NotFound) => {
            Err(ServerFnError::new("bookmark not found").into())
        }
        Err(db::bookmarks::BookmarkError::Sqlx(e)) => Err(internal_error("delete", e).into()),
        Err(e) => Err(internal_error("delete", e).into()),
    }
}
