//! On-demand external book-summary fetch backing the "Fetch Summary" button.
//! The client drives the cascade one source at a time (so it can show a
//! per-source status), calling [`rpc_summary_sources`] once for the ordered
//! plan (Hardcover → Google Books → Open Library, trimmed to the configured
//! providers) and then [`rpc_fetch_summary`] per source in that order.

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

/// The ordered summary-source cascade for the current settings (Hardcover when
/// keyed → Google Books always → Open Library only when no keys). Any
/// authenticated user may read this non-sensitive plan; it drives which sources
/// the client attempts, and in what order.
#[post("/api/rpc/ebook/summary/sources", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_summary_sources() -> Result<Vec<SummarySource>> {
    let sources = db::summary_source_plan(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("summary source plan", e))?;
    Ok(sources)
}
