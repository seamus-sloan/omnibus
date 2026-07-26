//! Per-user account server functions: the Send-to-Kindle destination address
//! and the self-service change-password flow, both editable from the Account
//! section of `/settings`.

use dioxus::fullstack::post;
use dioxus::prelude::*;

#[cfg(feature = "server")]
use omnibus_db::{self as db, auth::AuthError};

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Set (or clear, with `None`/blank) the authenticated user's Kindle email.
/// Rejects a malformed address with a validation `ServerFnError`.
#[post("/api/rpc/account/kindle-email", pool: PoolExt, user: AuthUser)]
pub async fn rpc_set_kindle_email(email: Option<String>) -> Result<()> {
    match db::auth::set_kindle_email(&pool.0, user.id, email.as_deref()).await {
        Ok(()) => Ok(()),
        Err(AuthError::Validation(msg)) => Err(ServerFnError::new(msg).into()),
        Err(e) => Err(internal_rpc_error("set kindle email", e).into()),
    }
}

/// Change the authenticated user's own password. The `AuthUser` extractor
/// scopes the change to the caller's own account (AC4). Surfaces a wrong
/// current password and a policy-failing new password as distinct client
/// errors the form renders inline; opaque failures fold to a generic message.
#[post("/api/rpc/account/change-password", pool: PoolExt, user: AuthUser)]
pub async fn rpc_change_password(current_password: String, new_password: String) -> Result<()> {
    match db::auth::change_password(
        &pool.0,
        user.id,
        &current_password,
        &new_password,
        user.session_id,
    )
    .await
    {
        Ok(()) => Ok(()),
        Err(AuthError::InvalidCredentials) => {
            Err(ServerFnError::new("current password is incorrect").into())
        }
        Err(AuthError::Validation(msg)) => Err(ServerFnError::new(msg).into()),
        Err(e) => Err(internal_rpc_error("change password", e).into()),
    }
}
