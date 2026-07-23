//! Reading-stats transport. Wraps the summary fetch for mobile
//! (`GET /api/stats` via reqwest) and web/SSR (the `rpc_stats` server
//! function). Both variants share the signature so the stats page stays
//! platform-agnostic; the `#[cfg]` gates carry the split.

use omnibus_shared::{StatsRange, StatsSummary};

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
