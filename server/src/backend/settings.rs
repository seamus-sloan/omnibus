//! `GET` / `POST /api/settings` handlers.
//!
//! Admin-gated read and write of the library-path settings KV. A successful
//! save dispatches a `Task::Scan` on the shared worker so the indexer
//! reflects any new library directory.

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{
    self as db,
    worker::{Task, TaskOutcome},
};
use omnibus_shared::Settings;

use super::{internal, AppState};
use crate::auth::AdminUser;

pub(super) async fn get_settings(_admin: AdminUser, State(state): State<AppState>) -> Response {
    match db::get_settings(&state.pool).await {
        Ok(settings) => Json(settings).into_response(),
        Err(error) => internal("read settings", error),
    }
}

pub(super) async fn post_settings(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(settings): Json<Settings>,
) -> Response {
    match db::set_settings(&state.pool, &settings).await {
        Ok(()) => match db::get_settings(&state.pool).await {
            Ok(updated) => {
                // Library path may have changed (and even when it hasn't,
                // the user has signalled they want to pick up on-disk
                // changes). Hand the reindex to the shared Worker so the
                // per-path mutex serializes overlapping saves and the
                // scan_concurrency cap stays honored.
                let task_id = updated
                    .ebook_library_path
                    .clone()
                    .map(|library_path| state.worker.post(Task::Scan { library_path }));

                let mut response = Json(updated).into_response();
                #[cfg(debug_assertions)]
                if let Some(id) = task_id {
                    if let Ok(value) = id.to_string().parse::<axum::http::HeaderValue>() {
                        response
                            .headers_mut()
                            .insert("X-Omnibus-Worker-Task-Id", value);
                    }
                }
                #[cfg(not(debug_assertions))]
                let _ = task_id;
                response
            }
            Err(error) => internal("read updated settings", error),
        },
        Err(error) => internal("save settings", error),
    }
}

/// Admin-only synchronous reindex: 200 on success, 409 when no library
/// path is configured, 500 on worker failure.
pub(super) async fn post_reindex(_admin: AdminUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    let Some(library_path) = settings.ebook_library_path else {
        return (
            axum::http::StatusCode::CONFLICT,
            "no ebook library path configured",
        )
            .into_response();
    };
    let task_id = state.worker.post(Task::Scan { library_path });
    match state.worker.await_completion(task_id).await {
        TaskOutcome::Ok => axum::http::StatusCode::OK.into_response(),
        TaskOutcome::Err(e) => internal("reindex", e),
    }
}

#[cfg(test)]
mod tests;
