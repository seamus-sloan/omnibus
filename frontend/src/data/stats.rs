//! Reading-stats transport. Wraps the per-user summary fetch and the
//! library-scale totals beside it for mobile (`GET /api/stats` and
//! `GET /api/library-size` via reqwest) and web/SSR (the matching server
//! functions). Both variants share their signatures so the stats page stays
//! platform-agnostic; the `#[cfg]` gates carry the split.

use omnibus_shared::{LibrarySize, StatsRange, StatsSummary};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// GET `/api/stats?range=…` — fetch the current user's stats summary.
#[cfg(feature = "mobile")]
pub async fn fetch_stats(server_url: &str, range: StatsRange) -> Result<StatsSummary, DataError> {
    let url = server_url.to_string();
    crate::offline::cache::read_through(
        crate::offline::cache::keys::stats(range.as_query()),
        async move { fetch_stats_online(&url, range).await },
    )
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn fetch_stats_online(
    server_url: &str,
    range: StatsRange,
) -> Result<StatsSummary, DataError> {
    let url = format!("{server_url}/api/stats?range={}", range.as_query());
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<StatsSummary>().await?)
}

/// Web/SSR `fetch_stats` — server-function wrapper that proxies to `rpc_stats`.
#[cfg(not(feature = "mobile"))]
pub async fn fetch_stats(_server_url: &str, range: StatsRange) -> Result<StatsSummary, DataError> {
    crate::rpc::rpc_stats(range)
        .await
        .map_err(note_server_fn_err)
}

/// GET `/api/library-size` — how big the library is in words, pages, and
/// hours of audio. Not per-user and not windowed, so it carries no query.
#[cfg(feature = "mobile")]
pub async fn fetch_library_size(server_url: &str) -> Result<LibrarySize, DataError> {
    let url = server_url.to_string();
    crate::offline::cache::read_through(crate::offline::cache::keys::library_size(), async move {
        let response = with_bearer(http_client().get(format!("{url}/api/library-size")))
            .send()
            .await?;
        let status = note_status(response.status());
        if !status.is_success() {
            return Err(drain_error(response, status).await);
        }
        Ok(response.json::<LibrarySize>().await?)
    })
    .await
}

/// Web/SSR `fetch_library_size` — proxies to `rpc_library_size`.
#[cfg(not(feature = "mobile"))]
pub async fn fetch_library_size(_server_url: &str) -> Result<LibrarySize, DataError> {
    crate::rpc::rpc_library_size()
        .await
        .map_err(note_server_fn_err)
}
