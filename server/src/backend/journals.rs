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
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, journals::JournalError};
use omnibus_shared::{CreateJournalEntry, UpdateJournalEntry, BODY_MAX_LEN};

use super::{internal, AppState};
use crate::auth::AuthUser;

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

/// List every user's journal entries for a book, newest first.
pub(super) async fn get_journal_entries(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(book_uuid): Path<String>,
) -> Response {
    match db::journals::list_journal_entries(&state.pool, &book_uuid).await {
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
