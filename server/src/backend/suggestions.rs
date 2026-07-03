//! F3.3 suggestions REST handlers (mobile-facing) plus the admin Hardcover-key
//! API; web hits the analogous `/api/rpc/*` server functions. Serves the
//! cache-backed "readers also enjoyed" strip (`GET .../suggestions`), streams
//! cached cover bytes (`GET .../cover`), and reads/writes the masked
//! account-level key (`GET`/`POST /api/hardcover-key`, admin-only).

use std::time::{SystemTime, UNIX_EPOCH};

use axum::{
    extract::{Path, State},
    http::{header, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, worker::Task};
use omnibus_shared::{BookSuggestion, RawSuggestion, SuggestionsResponse};
use serde::Deserialize;

use super::{internal, AppState};
use crate::auth::{AdminUser, AuthUser};

fn now_secs() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| i64::try_from(d.as_secs()).ok())
        .unwrap_or(0)
}

/// Serve a book's cached "readers also enjoyed" strip. Enqueues a single
/// background resolution (marking `pending` first) when the cache is
/// absent/stale so a refresh or a burst of viewers never re-hits Hardcover.
pub(super) async fn get_suggestions(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::effective_hardcover_api_key(&state.pool).await {
        Ok(None) => return Json(SuggestionsResponse::NotConfigured).into_response(),
        Ok(Some(_)) => {}
        Err(e) => return internal("read hardcover key", e),
    }
    let marker = match db::suggestion_state(&state.pool, &uuid).await {
        Ok(m) => m,
        Err(e) => return internal("read suggestion state", e),
    };
    let decision = db::decide(marker, now_secs());
    if decision.enqueue {
        if let Err(e) = db::mark_pending(&state.pool, &uuid).await {
            return internal("mark suggestions pending", e);
        }
        state.worker.post(Task::ResolveSuggestions {
            book_uuid: uuid.clone(),
        });
    }
    if !decision.serve {
        return Json(SuggestionsResponse::Pending).into_response();
    }
    match db::get_suggestions(&state.pool, &uuid).await {
        Ok(rows) => {
            let items = rows
                .into_iter()
                .map(|c| {
                    BookSuggestion::new(
                        &uuid,
                        c.rank,
                        RawSuggestion {
                            hardcover_id: c.hardcover_id,
                            hardcover_slug: c.hardcover_slug,
                            title: c.title,
                            author: c.author,
                            list_count: c.list_count,
                        },
                        c.has_cover,
                    )
                })
                .collect();
            Json(SuggestionsResponse::Ready { items }).into_response()
        }
        Err(e) => internal("read suggestions", e),
    }
}

/// Stream a cached suggestion cover's bytes; 404 on miss. Same cache semantics
/// as `/api/covers/{uuid}`.
pub(super) async fn get_suggestion_cover(
    _user: AuthUser,
    State(state): State<AppState>,
    Path((uuid, rank)): Path<(String, i64)>,
) -> Response {
    match db::get_suggestion_cover(&state.pool, &uuid, rank).await {
        Ok(Some((mime, bytes))) => (
            [
                (header::CONTENT_TYPE, mime.as_str()),
                (header::CACHE_CONTROL, "private, max-age=86400"),
                (header::VARY, "Cookie"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal("read suggestion cover", e),
    }
}

/// Admin-only: masked status of the server-wide Hardcover key.
pub(super) async fn get_hardcover_key(
    _admin: AdminUser,
    State(state): State<AppState>,
) -> Response {
    match db::hardcover_key_status(&state.pool).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => internal("read hardcover key", e),
    }
}

/// Body for `POST /api/hardcover-key`. A `null`/absent/blank key clears it.
#[derive(Debug, Deserialize)]
pub(super) struct SetHardcoverKey {
    #[serde(default)]
    key: Option<String>,
}

/// Admin-only: save or clear the Hardcover key; returns the new masked status.
/// A value over `HARDCOVER_API_KEY_MAX_LEN` returns 422 with the typed
/// validation message so an admin sees a per-case error rather than a 500.
pub(super) async fn post_hardcover_key(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(body): Json<SetHardcoverKey>,
) -> Response {
    match db::set_hardcover_api_key(&state.pool, body.key.as_deref()).await {
        Ok(()) => {}
        Err(db::SettingsError::Validation(msg)) => {
            return (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response();
        }
        Err(e) => return internal("save hardcover key", e),
    }
    match db::hardcover_key_status(&state.pool).await {
        Ok(s) => Json(s).into_response(),
        Err(e) => internal("read hardcover key", e),
    }
}

#[cfg(test)]
mod tests;
