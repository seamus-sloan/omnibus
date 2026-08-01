//! Highlight-annotation CRUD (create / list / recolor / note / delete).

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::{CreateHighlight, Highlight, HighlightColor, UpdateHighlightNote};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Create a highlight on a book. Mobile uses the analogous REST route in
/// `server::backend::highlights`; the rest of this family of RPCs follows
/// the same web-vs-mobile split.
#[post("/api/rpc/highlights/create", pool: PoolExt, user: AuthUser)]
pub async fn rpc_create_highlight(input: CreateHighlight) -> Result<Highlight> {
    if let Err(msg) = input.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    match db::annotations::create_highlight(&pool.0, user.id, &input).await {
        Ok(h) => {
            // Kobo down-sync opt-in: convert the fresh row in the background
            // so it reaches the device on its next sync.
            db::annotations::spawn_kobo_downsync_if_enabled(
                pool.0.clone(),
                user.id,
                h.book_uuid.clone(),
            );
            Ok(h)
        }
        Err(db::annotations::HighlightError::BookNotFound) => {
            Err(ServerFnError::new("book not found").into())
        }
        Err(db::annotations::HighlightError::NotFound) => {
            Err(ServerFnError::new("highlight not found").into())
        }
        Err(db::annotations::HighlightError::Sqlx(e)) => {
            Err(internal_rpc_error("create highlight", e).into())
        }
    }
}

/// List all highlights for the given book uuid, scoped to the current user
/// and ordered by creation time. Returns an empty list — not an error — when
/// the uuid is unknown or has no highlights yet.
#[post("/api/rpc/highlights/list", pool: PoolExt, user: AuthUser)]
pub async fn rpc_list_highlights(book_uuid: String) -> Result<Vec<Highlight>> {
    match db::annotations::list_highlights(&pool.0, user.id, &book_uuid).await {
        Ok(list) => Ok(list),
        Err(db::annotations::HighlightError::Sqlx(e)) => {
            Err(internal_rpc_error("list highlights", e).into())
        }
        Err(e) => Err(internal_rpc_error("list highlights", e).into()),
    }
}

/// Change the color of an existing highlight owned by the current user.
/// Errors with `"highlight not found"` when the id does not exist or
/// belongs to another user (the two cases are deliberately indistinguishable
/// to avoid leaking ownership).
#[post("/api/rpc/highlights/update-color", pool: PoolExt, user: AuthUser)]
pub async fn rpc_update_highlight_color(id: i64, color: HighlightColor) -> Result<()> {
    match db::annotations::update_highlight_color(&pool.0, user.id, id, color).await {
        Ok(()) => Ok(()),
        Err(db::annotations::HighlightError::NotFound) => {
            Err(ServerFnError::new("highlight not found").into())
        }
        Err(db::annotations::HighlightError::Sqlx(e)) => {
            Err(internal_rpc_error("update highlight color", e).into())
        }
        Err(e) => Err(internal_rpc_error("update highlight color", e).into()),
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
    match db::annotations::update_highlight_note(&pool.0, user.id, id, body.note.as_deref()).await {
        Ok(()) => Ok(()),
        Err(db::annotations::HighlightError::NotFound) => {
            Err(ServerFnError::new("highlight not found").into())
        }
        Err(db::annotations::HighlightError::Sqlx(e)) => {
            Err(internal_rpc_error("update highlight note", e).into())
        }
        Err(e) => Err(internal_rpc_error("update highlight note", e).into()),
    }
}

/// Delete a highlight owned by the current user. Errors with
/// `"highlight not found"` when the id does not exist or belongs to another
/// user (the two cases are deliberately indistinguishable).
#[post("/api/rpc/highlights/delete", pool: PoolExt, user: AuthUser)]
pub async fn rpc_delete_highlight(id: i64) -> Result<()> {
    match db::annotations::delete_highlight(&pool.0, user.id, id).await {
        Ok(()) => Ok(()),
        Err(db::annotations::HighlightError::NotFound) => {
            Err(ServerFnError::new("highlight not found").into())
        }
        Err(db::annotations::HighlightError::Sqlx(e)) => {
            Err(internal_rpc_error("delete highlight", e).into())
        }
        Err(e) => Err(internal_rpc_error("delete highlight", e).into()),
    }
}
