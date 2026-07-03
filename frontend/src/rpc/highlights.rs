//! Highlight-annotation CRUD (create / list / recolor / note / delete).

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::{CreateHighlight, Highlight, HighlightColor, UpdateHighlightNote};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{AuthUser, PoolExt};

/// Create a highlight on a book. Mobile uses the analogous REST route in
/// `server::backend::highlights`; the rest of this family of RPCs follows
/// the same web-vs-mobile split.
#[post("/api/rpc/highlights/create", pool: PoolExt, user: AuthUser)]
pub async fn rpc_create_highlight(input: CreateHighlight) -> Result<Highlight> {
    if let Err(msg) = input.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    match db::highlights::create_highlight(&pool.0, user.id, &input).await {
        Ok(h) => Ok(h),
        Err(db::highlights::HighlightError::BookNotFound) => {
            Err(ServerFnError::new("book not found").into())
        }
        Err(db::highlights::HighlightError::NotFound) => {
            Err(ServerFnError::new("highlight not found").into())
        }
        Err(db::highlights::HighlightError::Sqlx(e)) => {
            Err(ServerFnError::new(e.to_string()).into())
        }
    }
}

/// List all highlights for the given book uuid, scoped to the current user
/// and ordered by creation time. Returns an empty list — not an error — when
/// the uuid is unknown or has no highlights yet.
#[post("/api/rpc/highlights/list", pool: PoolExt, user: AuthUser)]
pub async fn rpc_list_highlights(book_uuid: String) -> Result<Vec<Highlight>> {
    match db::highlights::list_highlights(&pool.0, user.id, &book_uuid).await {
        Ok(list) => Ok(list),
        Err(db::highlights::HighlightError::Sqlx(e)) => {
            Err(ServerFnError::new(e.to_string()).into())
        }
        Err(e) => Err(ServerFnError::new(e.to_string()).into()),
    }
}

/// Change the color of an existing highlight owned by the current user.
/// Errors with `"highlight not found"` when the id does not exist or
/// belongs to another user (the two cases are deliberately indistinguishable
/// to avoid leaking ownership).
#[post("/api/rpc/highlights/update-color", pool: PoolExt, user: AuthUser)]
pub async fn rpc_update_highlight_color(id: i64, color: HighlightColor) -> Result<()> {
    match db::highlights::update_highlight_color(&pool.0, user.id, id, color).await {
        Ok(()) => Ok(()),
        Err(db::highlights::HighlightError::NotFound) => {
            Err(ServerFnError::new("highlight not found").into())
        }
        Err(db::highlights::HighlightError::Sqlx(e)) => {
            Err(ServerFnError::new(e.to_string()).into())
        }
        Err(e) => Err(ServerFnError::new(e.to_string()).into()),
    }
}

/// Set or clear the free-text note on a highlight owned by the current
/// user. Validates `body` against `UpdateHighlightNote::validate()` (note
/// length cap) and errors with `"highlight not found"` when the id does
/// not exist or belongs to another user.
#[post("/api/rpc/highlights/update-note", pool: PoolExt, user: AuthUser)]
pub async fn rpc_update_highlight_note(id: i64, body: UpdateHighlightNote) -> Result<()> {
    if let Err(msg) = body.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    match db::highlights::update_highlight_note(&pool.0, user.id, id, body.note.as_deref()).await {
        Ok(()) => Ok(()),
        Err(db::highlights::HighlightError::NotFound) => {
            Err(ServerFnError::new("highlight not found").into())
        }
        Err(db::highlights::HighlightError::Sqlx(e)) => {
            Err(ServerFnError::new(e.to_string()).into())
        }
        Err(e) => Err(ServerFnError::new(e.to_string()).into()),
    }
}

/// Delete a highlight owned by the current user. Errors with
/// `"highlight not found"` when the id does not exist or belongs to another
/// user (the two cases are deliberately indistinguishable).
#[post("/api/rpc/highlights/delete", pool: PoolExt, user: AuthUser)]
pub async fn rpc_delete_highlight(id: i64) -> Result<()> {
    match db::highlights::delete_highlight(&pool.0, user.id, id).await {
        Ok(()) => Ok(()),
        Err(db::highlights::HighlightError::NotFound) => {
            Err(ServerFnError::new("highlight not found").into())
        }
        Err(db::highlights::HighlightError::Sqlx(e)) => {
            Err(ServerFnError::new(e.to_string()).into())
        }
        Err(e) => Err(ServerFnError::new(e.to_string()).into()),
    }
}
