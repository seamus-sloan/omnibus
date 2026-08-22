//! Admin "last errors" server function (`/api/rpc/errors`). Web-only: reads the
//! in-memory error ring buffer via `db::error_ring` and returns its full
//! contents, newest first. Admin-gated by the `AdminUser` extractor, so a
//! non-admin session is rejected before the buffer is touched.

use dioxus::fullstack::get;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use omnibus_db as db;
use omnibus_shared::error_ring::CapturedError;

#[cfg(feature = "server")]
use super::{AdminUser, PoolExt};

/// Admin-only: snapshot of the in-memory error ring buffer, newest first.
/// `_pool` is unused — the buffer lives in process memory, not the DB — but
/// kept so this route lists the pool extension identically to every other
/// `/api/rpc/*` route.
#[get("/api/rpc/errors", _pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_get_last_errors() -> Result<Vec<CapturedError>> {
    Ok(db::error_ring::snapshot())
}
