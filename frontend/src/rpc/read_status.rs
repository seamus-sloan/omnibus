//! Per-user read/unread state set / fetch.

use dioxus::fullstack::post;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use omnibus_db as db;
use omnibus_shared::{ReadStatusRecord, SetReadStatus};

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Set the current user's read state for a book. Mobile uses the analogous
/// REST route in `server::backend::read_status`. POST carries the
/// `SetReadStatus` body — same rationale as `rpc_save_progress`.
#[post("/api/rpc/read-status/set", pool: PoolExt, user: AuthUser)]
pub async fn rpc_set_read_status(update: SetReadStatus) -> Result<ReadStatusRecord> {
    if let Err(msg) = update.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    match db::read_status::set_read_status(&pool.0, user.id, &update).await {
        Ok(rec) => Ok(rec),
        Err(db::read_status::ReadStatusError::BookNotFound) => {
            Err(ServerFnError::new("book not found").into())
        }
        Err(db::read_status::ReadStatusError::Sqlx(e)) => {
            Err(internal_rpc_error("set read status", e).into())
        }
    }
}

/// Fetch the current user's read state for a book. Returns `Ok(None)` when the
/// book is unknown or the user has no row yet (the caller treats that as
/// `unread`).
#[post("/api/rpc/read-status/get", pool: PoolExt, user: AuthUser)]
pub async fn rpc_get_read_status(uuid: String) -> Result<Option<ReadStatusRecord>> {
    Ok(db::read_status::get_read_status(&pool.0, user.id, &uuid)
        .await
        .map_err(|e| internal_rpc_error("get read status", e))?)
}
