//! Admin device & session management REST handlers (F5.4). Every route is
//! `AdminUser`-gated (403 for non-admins): list a target user's sessions and
//! devices, and revoke a specific session or device. Companion to
//! `server::auth::handlers`, which owns the self-service equivalents under
//! `/api/auth/sessions`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, auth::AuthError};
use omnibus_shared::{DeviceView, SessionView};

use super::{internal, AppState};
use crate::auth::AdminUser;

/// Project a db session row to its wire view. `is_current` is always `false`
/// here — this admin listing never *sets* the flag (unlike the self-service
/// listing in `auth::handlers`), not because the viewed row can never be the
/// admin's own session: an admin listing their own account can land on the
/// exact session their request authenticated with, same as any other row.
/// See [`SessionView`] docs.
fn to_session_view(s: db::auth::Session) -> SessionView {
    SessionView {
        id: s.id,
        device_id: s.device_id,
        kind: s.kind.as_str().to_string(),
        created_at: s.created_at,
        last_used_at: s.last_used_at,
        expires_at: s.expires_at,
        is_current: false,
    }
}

fn to_device_view(d: db::auth::Device) -> DeviceView {
    DeviceView {
        id: d.id,
        name: d.name,
        client_kind: d.client_kind,
        client_version: d.client_version,
        created_at: d.created_at,
        last_seen_at: d.last_seen_at,
    }
}

/// `GET /api/admin/users/{id}/sessions` — list a target user's live sessions.
pub(super) async fn get_user_sessions(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
) -> Response {
    match db::auth::list_sessions_for_user(&state.pool, user_id).await {
        Ok(rows) => Json(rows.into_iter().map(to_session_view).collect::<Vec<_>>()).into_response(),
        Err(e) => internal("admin list user sessions", e),
    }
}

/// `GET /api/admin/users/{id}/devices` — list a target user's registered devices.
pub(super) async fn get_user_devices(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
) -> Response {
    match db::auth::list_devices_for_user(&state.pool, user_id).await {
        Ok(rows) => Json(rows.into_iter().map(to_device_view).collect::<Vec<_>>()).into_response(),
        Err(e) => internal("admin list user devices", e),
    }
}

/// `DELETE /api/admin/sessions/{id}` — revoke any user's session. Refuses to
/// revoke the session the request itself authenticated with, mirroring the
/// self-service guard in `auth::handlers::delete_session_handler` (an admin
/// panel is not exempt from locking its own tab out mid-request). `404` when
/// the id is unknown or already revoked.
pub(super) async fn delete_session(
    admin: AdminUser,
    State(state): State<AppState>,
    Path(session_id): Path<i64>,
) -> Response {
    if session_id == admin.0.session_id {
        return (
            StatusCode::BAD_REQUEST,
            "cannot revoke the currently-active session",
        )
            .into_response();
    }
    match db::auth::revoke_session_checked(&state.pool, session_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(AuthError::SessionNotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal("admin revoke session", e),
    }
}

/// `DELETE /api/admin/devices/{id}` — revoke any user's device registration
/// (and any session still tied to it). `404` when the id is unknown.
pub(super) async fn delete_device(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(device_id): Path<i64>,
) -> Response {
    match db::auth::revoke_device(&state.pool, device_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(AuthError::DeviceNotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal("admin revoke device", e),
    }
}

#[cfg(test)]
mod tests;
