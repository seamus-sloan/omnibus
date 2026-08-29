//! Per-user API-token server functions: list, create, and revoke the
//! long-lived `omni_…` bearers managed from the Settings → API Tokens
//! section. Every function is scoped to the authenticated `AuthUser`, so one
//! user's request can only touch their own tokens. The create response is the
//! only surface that ever carries the raw secret (shown once, never stored).

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::{ApiTokenView, CreateApiTokenResponse};

#[cfg(feature = "server")]
use omnibus_db::{self as db, auth::AuthError};

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Project a db row to its wire view (no hash, no secret).
#[cfg(feature = "server")]
fn to_view(t: db::auth::ApiToken) -> ApiTokenView {
    ApiTokenView {
        id: t.id,
        name: t.name,
        created_at: t.created_at,
        last_used_at: t.last_used_at,
        suffix: t.suffix,
    }
}

/// List the caller's live API tokens, newest first.
#[post("/api/rpc/api-tokens", pool: PoolExt, user: AuthUser)]
pub async fn rpc_list_api_tokens() -> Result<Vec<ApiTokenView>> {
    match db::auth::list_api_tokens_for_user(&pool.0, user.id).await {
        Ok(list) => Ok(list.into_iter().map(to_view).collect()),
        Err(e) => Err(internal_rpc_error("list api tokens", e).into()),
    }
}

/// Mint a new API token. The returned `secret` is displayed exactly once —
/// the server keeps only its hash (AC2).
#[post("/api/rpc/api-tokens/create", pool: PoolExt, user: AuthUser)]
pub async fn rpc_create_api_token(name: String) -> Result<CreateApiTokenResponse> {
    match db::auth::create_api_token(&pool.0, user.id, &name).await {
        Ok(minted) => Ok(CreateApiTokenResponse {
            token: to_view(minted.token),
            secret: minted.raw_token,
        }),
        Err(AuthError::Validation(msg)) => Err(ServerFnError::new(msg).into()),
        Err(e) => Err(internal_rpc_error("create api token", e).into()),
    }
}

/// Rename one of the caller's API tokens (AC3). Same not-found opacity as
/// revoke: unknown, revoked, and another user's ids are indistinguishable.
#[post("/api/rpc/api-tokens/rename", pool: PoolExt, user: AuthUser)]
pub async fn rpc_rename_api_token(id: i64, name: String) -> Result<()> {
    match db::auth::rename_api_token_for_user(&pool.0, user.id, id, &name).await {
        Ok(()) => Ok(()),
        Err(AuthError::Validation(msg)) => Err(ServerFnError::new(msg).into()),
        Err(AuthError::ApiTokenNotFound) => Err(ServerFnError::new("api token not found").into()),
        Err(e) => Err(internal_rpc_error("rename api token", e).into()),
    }
}

/// Revoke one of the caller's API tokens; requests bearing it fail
/// immediately afterward (AC3).
#[post("/api/rpc/api-tokens/revoke", pool: PoolExt, user: AuthUser)]
pub async fn rpc_revoke_api_token(id: i64) -> Result<()> {
    match db::auth::revoke_api_token_for_user(&pool.0, user.id, id).await {
        Ok(()) => Ok(()),
        Err(AuthError::ApiTokenNotFound) => Err(ServerFnError::new("api token not found").into()),
        Err(e) => Err(internal_rpc_error("revoke api token", e).into()),
    }
}
