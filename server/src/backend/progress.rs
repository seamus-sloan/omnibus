//! Progress-sync REST handlers for the mobile client (`/api/progress*`).
//!
//! Accepts a discriminated `{ format: "epub" | "audio" }` payload and fans
//! out to per-format write paths in `omnibus_db::progress`. Web clients use
//! the `/api/rpc/progress*` server functions in `omnibus_frontend::rpc`.

use axum::{
    extract::{Path, Query, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, progress::ProgressError};
use omnibus_shared::{ProgressFormat, ProgressUpdate, SessionReport, SESSION_BATCH_CAP};
use serde::Deserialize;

use super::{internal, AppState};
use crate::auth::AuthUser;

#[derive(Debug, Deserialize)]
pub(super) struct ProgressQuery {
    #[serde(default = "default_format")]
    format: ProgressFormat,
}

fn default_format() -> ProgressFormat {
    ProgressFormat::Epub
}

/// Persist a new reading/listening position. Last-write-wins on
/// `(user, book, format)`; returns the server-authoritative record so the
/// caller can sync forward.
pub(super) async fn post_progress(
    user: AuthUser,
    State(state): State<AppState>,
    Json(update): Json<ProgressUpdate>,
) -> Response {
    if let Err(msg) = update.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
    }
    match db::progress::upsert_progress(&state.pool, user.id, &update).await {
        Ok(rec) => Json(rec).into_response(),
        Err(ProgressError::BookNotFound) => {
            (axum::http::StatusCode::NOT_FOUND, "book not found").into_response()
        }
        Err(ProgressError::Sqlx(e)) => internal("upsert_progress", e),
    }
}

/// Fetch the current position for `(user, uuid, format)`. `format` defaults
/// to `epub` when omitted. Returns `200 { … }` with an `Option<ProgressRecord>`
/// body (`null` when the user has not yet opened the book in that format).
pub(super) async fn get_progress(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Query(q): Query<ProgressQuery>,
) -> Response {
    match db::progress::get_progress(&state.pool, user.id, &uuid, q.format).await {
        Ok(rec) => Json(rec).into_response(),
        Err(e) => internal("get_progress", e),
    }
}

/// Append a batch of session reports. Mobile posts these on reconnect; web
/// posts best-effort on unmount. Each report is validated at the API
/// boundary (negative durations / inverted time ranges → 400); unknown
/// book uuids are silently skipped inside the db layer (best-effort
/// telemetry). `recorded` reflects the **inserted** row count so callers
/// can tell which queued reports actually persisted. Batches larger than
/// `SESSION_BATCH_CAP` (defined in `omnibus_shared` so the web RPC path
/// in `omnibus_frontend::rpc::rpc_record_sessions` enforces the same
/// bound) are rejected with 422 before any DB work.
///
/// The entire batch runs inside a single transaction — a DB error mid-loop
/// rolls back all previously inserted rows so the client can safely retry
/// without risking double-counts.
pub(super) async fn post_sessions(
    user: AuthUser,
    State(state): State<AppState>,
    Json(reports): Json<Vec<SessionReport>>,
) -> Response {
    if reports.len() > SESSION_BATCH_CAP {
        let msg = format!(
            "batch too large: {} records exceeds maximum of {}",
            reports.len(),
            SESSION_BATCH_CAP
        );
        return (axum::http::StatusCode::UNPROCESSABLE_ENTITY, msg).into_response();
    }
    for r in &reports {
        if let Err(msg) = r.validate() {
            return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
        }
    }
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(e) => return internal("begin", e),
    };
    let mut inserted = 0usize;
    for r in &reports {
        match db::progress::record_session_tx(&mut tx, user.id, r).await {
            Ok(true) => inserted += 1,
            Ok(false) => {}
            Err(e) => return internal("record_session", e),
        }
    }
    if let Err(e) = tx.commit().await {
        return internal("commit", e);
    }
    Json(serde_json::json!({ "recorded": inserted })).into_response()
}

#[cfg(test)]
mod tests;
