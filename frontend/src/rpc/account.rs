//! Per-user account server functions: the display name, the Send-to-Kindle
//! destination address, and the self-service change-password flow, all
//! editable from `/settings`. The avatar upload is REST-only — multipart
//! doesn't fit a server function, so it follows the author-photo precedent.

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

/// Set (or clear, with `None`/blank) the authenticated user's display name.
/// The db layer renames their Wishlist shelf in the same transaction, so the
/// shelf label can't drift from the name it was derived from.
#[post("/api/rpc/account/profile", pool: PoolExt, user: AuthUser)]
pub async fn rpc_set_display_name(display_name: Option<String>) -> Result<()> {
    match db::auth::set_display_name(&pool.0, user.id, display_name.as_deref()).await {
        Ok(()) => Ok(()),
        Err(AuthError::Validation(msg)) => Err(ServerFnError::new(msg).into()),
        Err(e) => Err(internal_rpc_error("set display name", e).into()),
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
