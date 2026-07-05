//! Per-user account server functions. Currently just the F4.3 Send-to-Kindle
//! destination address, editable from the `/account` page.

use dioxus::fullstack::post;
use dioxus::prelude::*;

#[cfg(feature = "server")]
use omnibus_db::{self as db, auth::AuthError};

#[cfg(feature = "server")]
use super::{AuthUser, PoolExt};

/// Set (or clear, with `None`/blank) the authenticated user's Kindle email.
/// Rejects a malformed address with a validation `ServerFnError`.
#[post("/api/rpc/account/kindle-email", pool: PoolExt, user: AuthUser)]
pub async fn rpc_set_kindle_email(email: Option<String>) -> Result<()> {
    match db::auth::set_kindle_email(&pool.0, user.id, email.as_deref()).await {
        Ok(()) => Ok(()),
        Err(AuthError::Validation(msg)) => Err(ServerFnError::new(msg).into()),
        Err(e) => Err(ServerFnError::new(e.to_string()).into()),
    }
}
