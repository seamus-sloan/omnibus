//! `/api/auth/api-tokens` handlers: the self-service create/list/revoke
//! surface for long-lived API tokens. Every route is scoped to the
//! authenticated caller — one user's request can only see or revoke their
//! own tokens. The create response is the only place the raw secret ever
//! appears (AC2); the listing carries names and timestamps, never secrets.

use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::auth::{self as auth_db, ApiToken, AuthError};
use omnibus_shared::{ApiTokenView, CreateApiTokenRequest, CreateApiTokenResponse};

use super::extractor::AuthUser;
use crate::backend::AppState;
use crate::http_errors::internal;

/// Project a db token row to its wire view (no hash, no secret).
fn to_view(t: ApiToken) -> ApiTokenView {
    ApiTokenView {
        id: t.id,
        name: t.name,
        created_at: t.created_at,
        last_used_at: t.last_used_at,
    }
}

/// `GET /api/auth/api-tokens` — list the caller's live API tokens.
pub async fn get_api_tokens_handler(user: AuthUser, State(state): State<AppState>) -> Response {
    match auth_db::list_api_tokens_for_user(state.pool(), user.id).await {
        Ok(rows) => Json(rows.into_iter().map(to_view).collect::<Vec<_>>()).into_response(),
        Err(e) => internal("list api tokens", e),
    }
}

/// `POST /api/auth/api-tokens` — mint a new token for the caller. The
/// response's `secret` is shown exactly once; only its hash is stored.
pub async fn post_api_token_handler(
    user: AuthUser,
    State(state): State<AppState>,
    Json(req): Json<CreateApiTokenRequest>,
) -> Response {
    match auth_db::create_api_token(state.pool(), user.id, &req.name).await {
        Ok(minted) => Json(CreateApiTokenResponse {
            token: to_view(minted.token),
            secret: minted.raw_token,
        })
        .into_response(),
        Err(AuthError::Validation(msg)) => (StatusCode::BAD_REQUEST, msg).into_response(),
        Err(e) => internal("create api token", e),
    }
}

/// `DELETE /api/auth/api-tokens/{id}` — revoke one of the caller's tokens.
/// `404` when the id is unknown, already revoked, or another user's (the
/// three are indistinguishable on the wire).
pub async fn delete_api_token_handler(
    user: AuthUser,
    State(state): State<AppState>,
    Path(token_id): Path<i64>,
) -> Response {
    match auth_db::revoke_api_token_for_user(state.pool(), user.id, token_id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(AuthError::ApiTokenNotFound) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal("revoke api token", e),
    }
}

#[cfg(test)]
mod tests;
