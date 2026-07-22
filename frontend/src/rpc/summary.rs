//! On-demand external book-summary fetch backing the "Fetch Summary" button.
//! The client drives the Hardcover→OpenLibrary cascade one source at a time
//! (so it can show a per-source status), calling [`rpc_fetch_summary`] per
//! source and [`rpc_hardcover_configured`] to decide whether to start with
//! Hardcover.

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::summary::SummarySource;

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Fetch a summary for a book from `source`. Requires `can_edit` or admin.
/// Returns the fetched text, or `None` when that source has no summary (a
/// clean miss the client cascades past). External network failures surface as
/// an opaque internal error.
#[post("/api/rpc/ebook/summary/fetch", pool: PoolExt, user: AuthUser)]
pub async fn rpc_fetch_summary(uuid: String, source: SummarySource) -> Result<Option<String>> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    let text = db::fetch_summary(&pool.0, &uuid, source)
        .await
        .map_err(|e| internal_rpc_error("fetch summary", e))?;
    Ok(text)
}

/// Whether a server-wide Hardcover key is configured. Any authenticated user
/// may read this non-sensitive flag; it drives the client's source cascade
/// (Hardcover first when configured, else OpenLibrary).
#[post("/api/rpc/ebook/summary/hardcover-configured", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_hardcover_configured() -> Result<bool> {
    let status = db::hardcover_key_status(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("hardcover key status", e))?;
    Ok(status.configured)
}
