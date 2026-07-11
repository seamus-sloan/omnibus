//! Public per-book journal REST handlers for the mobile client
//! (`/api/journals*`).
//!
//! Create/list/edit/delete free-form markdown journal entries on a book.
//! Entries are public — `GET /api/journals/book/{uuid}` returns every user's
//! entries — but edit/delete are owner-scoped (404 for a non-owner, same as a
//! missing row). The body is validated at the boundary (non-empty, size cap,
//! progress range); markdown is rendered to sanitized HTML server-side. Web
//! clients use the `/api/rpc/journals/*` server functions in
//! `omnibus_frontend::rpc`.

use axum::{
    extract::{Path, State},
    http::header,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, journals::JournalError};
use omnibus_shared::{CreateJournalEntry, JournalImageUpload, UpdateJournalEntry, BODY_MAX_LEN};

use super::{image_upload, internal, AppState};
use crate::auth::{AuthUser, MediaAuthUser};

/// Create a journal entry. 400 on an empty/oversized body or out-of-range
/// progress, 404 for a book the server has never indexed.
pub(super) async fn post_journal(
    user: AuthUser,
    State(state): State<AppState>,
    Json(input): Json<CreateJournalEntry>,
) -> Response {
    if let Err(msg) = input.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
    }
    match db::journals::create_journal_entry(&state.pool, user.id, &input).await {
        Ok(entry) => Json(entry).into_response(),
        Err(JournalError::BookNotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "book not found").into_response()
        }
        Err(e @ JournalError::NotFound) => internal("create_journal_entry", e),
        Err(JournalError::Sqlx(e)) => internal("create_journal_entry", e),
    }
}

/// List every user's published journal entries for a book — plus the caller's
/// own drafts — newest first.
pub(super) async fn get_journal_entries(
    user: AuthUser,
    State(state): State<AppState>,
    Path(book_uuid): Path<String>,
) -> Response {
    match db::journals::list_journal_entries(&state.pool, user.id, &book_uuid).await {
        Ok(list) => Json(list).into_response(),
        Err(e) => internal("list_journal_entries", e),
    }
}

/// Edit a journal entry owned by the current user. 400 on an invalid body, 404
/// when the id does not exist or belongs to another user.
pub(super) async fn patch_journal(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(input): Json<UpdateJournalEntry>,
) -> Response {
    if let Err(msg) = input.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
    }
    match db::journals::update_journal_entry(&state.pool, user.id, id, &input).await {
        Ok(entry) => Json(entry).into_response(),
        Err(JournalError::NotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "journal entry not found").into_response()
        }
        Err(e @ JournalError::BookNotFound) => internal("update_journal_entry", e),
        Err(JournalError::Sqlx(e)) => internal("update_journal_entry", e),
    }
}

/// Render a draft journal body to sanitized HTML for the composer preview,
/// using the same renderer as the persisted read path. The request and response
/// bodies are both bare JSON strings. Rejects bodies over `BODY_MAX_LEN` so an
/// authenticated client can't drive arbitrary markdown+sanitize work.
pub(super) async fn post_journal_preview(_user: AuthUser, Json(body_md): Json<String>) -> Response {
    if body_md.len() > BODY_MAX_LEN {
        return (
            axum::http::StatusCode::BAD_REQUEST,
            format!("journal entry must be {BODY_MAX_LEN} bytes or fewer"),
        )
            .into_response();
    }
    Json(db::journals::markdown::render(&body_md)).into_response()
}

/// Upload an image to embed in a journal entry. Multipart form with a single
/// `image` field; validated (content-type, SVG rejection, 10 MiB cap, magic
/// bytes) by the shared image-upload pipeline, then stored under a
/// server-minted name. Returns the serving URL to reference from markdown.
pub(super) async fn post_journal_image(
    _user: AuthUser,
    mut multipart: axum::extract::Multipart,
) -> Response {
    let (mime, bytes) = match image_upload::extract_validated_image(&mut multipart, "image").await {
        Ok(pair) => pair,
        Err(response) => return response,
    };
    // Sync `std::fs` write — run on the blocking pool (#106).
    match tokio::task::spawn_blocking(move || {
        db::journal_images::write_journal_image(&mime, &bytes)
    })
    .await
    {
        Ok(Ok(name)) => Json(JournalImageUpload {
            url: format!("{}{name}", db::journals::markdown::IMAGE_URL_PREFIX),
        })
        .into_response(),
        Ok(Err(e)) => internal("write_journal_image", e),
        Err(e) => internal("spawn_blocking(write_journal_image)", e),
    }
}

/// Serve a stored journal image by its minted name. 404 for a malformed name
/// (the traversal guard) or a missing file. Media-gated like covers/thumbs.
pub(super) async fn get_journal_image(_user: MediaAuthUser, Path(name): Path<String>) -> Response {
    // Sync `std::fs` read — run on the blocking pool (#106).
    match tokio::task::spawn_blocking(move || db::journal_images::read_journal_image(&name)).await {
        Ok(Some((mime, bytes))) => (
            [
                (header::CONTENT_TYPE, mime),
                // Names are content-unique (uuidv4, never rewritten), so long
                // client-side caching is safe; `private` + `Vary: Cookie` keep
                // shared proxies from serving them across sessions (mirrors
                // covers).
                (
                    header::CACHE_CONTROL,
                    "private, max-age=31536000, immutable",
                ),
                (header::VARY, "Cookie"),
                // Prevent MIME-sniffing an image into an executable type.
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "image not found").into_response(),
        Err(e) => internal("spawn_blocking(read_journal_image)", e),
    }
}

/// Delete a journal entry owned by the current user. 404 when the id does not
/// exist or belongs to another user.
pub(super) async fn delete_journal(
    user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::journals::delete_journal_entry(&state.pool, user.id, id).await {
        Ok(()) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(JournalError::NotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "journal entry not found").into_response()
        }
        Err(e @ JournalError::BookNotFound) => internal("delete_journal_entry", e),
        Err(JournalError::Sqlx(e)) => internal("delete_journal_entry", e),
    }
}

#[cfg(test)]
mod tests;
