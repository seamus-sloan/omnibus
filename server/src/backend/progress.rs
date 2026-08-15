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

#[derive(Debug, Deserialize)]
pub(super) struct RecentQuery {
    #[serde(default = "default_recent_limit")]
    limit: i64,
}

fn default_recent_limit() -> i64 {
    1
}

/// Ceiling on `?limit=` for `GET /api/progress/recent` — the surface is a
/// small "pick up where you left off" strip, not a history browser.
const RECENT_LIMIT_CAP: i64 = 20;

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

/// The user's most recent progress rows joined with their books — the
/// mobile "pick up where you left off" feed. `limit` defaults to 1 and is
/// capped at [`RECENT_LIMIT_CAP`]. Rows whose book has vanished are skipped
/// in the db layer.
pub(super) async fn get_recent_progress(
    user: AuthUser,
    State(state): State<AppState>,
    Query(q): Query<RecentQuery>,
) -> Response {
    let limit = q.limit.clamp(1, RECENT_LIMIT_CAP);
    match db::progress::resume_points(&state.pool, user.id, limit).await {
        Ok(points) => Json(points).into_response(),
        Err(e) => internal("resume_points", e),
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
    // BEGIN IMMEDIATE avoids a stale-snapshot 517 on concurrent batches (#1862).
    let mut tx = match state.pool.begin_with("BEGIN IMMEDIATE").await {
        Ok(tx) => tx,
        Err(e) => return internal("begin", e),
    };
    // Pre-resolve every uuid in the batch in one round-trip (chunked at 499)
    // so the insert loop below performs only INSERTs — no per-row
    // `SELECT ... FROM books UNION merged_uuids` fires. Worst-case shrinks
    // from 2N to `chunks + N` queries per request (issue #633).
    let batch_uuids: Vec<String> = reports.iter().map(|r| r.book_uuid.clone()).collect();
    let resolved = match db::resolve_canonical_book_uuids_bulk_exec(&mut tx, &batch_uuids).await {
        Ok(m) => m,
        Err(e) => return internal("resolve_bulk", e),
    };
    let mut inserted = 0usize;
    for r in &reports {
        // Missing entry = uuid unknown in both `books` and `merged_uuids`.
        // Same best-effort skip semantics `record_session_tx` used to enforce
        // with its per-row resolve — the row is silently dropped so the
        // client's `recorded` count reflects what actually landed.
        let Some(canonical) = resolved.get(&r.book_uuid) else {
            continue;
        };
        if let Err(e) = db::progress::insert_session_tx(&mut tx, user.id, r, canonical).await {
            return internal("insert_session", e);
        }
        inserted += 1;
    }
    if let Err(e) = tx.commit().await {
        return internal("commit", e);
    }
    Json(serde_json::json!({ "recorded": inserted })).into_response()
}

#[cfg(test)]
mod tests;
