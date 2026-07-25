//! Admin user-management REST handlers (F5.4). Every route is `AdminUser`-gated
//! (403 for non-admins): list, create, update permissions, reset password,
//! unlock, and delete — with the last-admin guardrails enforced by the db
//! layer surfaced as `409 Conflict`.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db, auth::AuthError};
use omnibus_shared::{CreateUserRequest, SetPasswordRequest, UserPermissions};

use super::{internal, AppState};
use crate::auth::AdminUser;

/// `GET /api/users` — list every user for the admin table.
pub(super) async fn get_users(_admin: AdminUser, State(state): State<AppState>) -> Response {
    match db::auth::list_users(&state.pool).await {
        Ok(rows) => Json(rows).into_response(),
        Err(e) => internal("list users", e),
    }
}

/// `POST /api/users` — create a user with explicit permissions. `201` with the
/// new row; `409` on a duplicate username; `422` on username/password policy.
pub(super) async fn post_user(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(req): Json<CreateUserRequest>,
) -> Response {
    match db::auth::admin_create_user(&state.pool, &req.username, &req.password, req.permissions)
        .await
    {
        Ok(row) => (StatusCode::CREATED, Json(row)).into_response(),
        Err(AuthError::UsernameTaken) => {
            (StatusCode::CONFLICT, "username is already taken").into_response()
        }
        Err(AuthError::Validation(msg)) => (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response(),
        Err(e) => internal("create user", e),
    }
}

/// `PATCH /api/users/{id}/permissions` — replace the four permission flags.
/// `404` for an unknown id; `409` when it would demote the last admin.
pub(super) async fn patch_permissions(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(perms): Json<UserPermissions>,
) -> Response {
    match db::auth::update_user_permissions(&state.pool, id, perms).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(AuthError::UserNotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(AuthError::LastAdmin) => {
            (StatusCode::CONFLICT, "cannot remove the last administrator").into_response()
        }
        Err(e) => internal("update user permissions", e),
    }
}

/// `POST /api/users/{id}/password` — admin password reset. `404` for an unknown
/// id; `422` when the new password fails policy.
pub(super) async fn post_password(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(req): Json<SetPasswordRequest>,
) -> Response {
    match db::auth::admin_set_password(&state.pool, id, &req.password).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(AuthError::UserNotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(AuthError::Validation(msg)) => (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response(),
        Err(e) => internal("reset user password", e),
    }
}

/// `POST /api/users/{id}/unlock` — clear a repeated-failed-login lockout. `404`
/// for an unknown id.
pub(super) async fn post_unlock(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::auth::unlock_user(&state.pool, id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(AuthError::UserNotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal("unlock user", e),
    }
}

/// `DELETE /api/users/{id}` — delete a user (their owned rows cascade). `404`
/// for an unknown id; `409` when it would delete the last admin.
pub(super) async fn delete_user(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::auth::delete_user(&state.pool, id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(AuthError::UserNotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(AuthError::LastAdmin) => {
            (StatusCode::CONFLICT, "cannot delete the last administrator").into_response()
        }
        Err(e) => internal("delete user", e),
    }
}

#[cfg(test)]
mod tests;
