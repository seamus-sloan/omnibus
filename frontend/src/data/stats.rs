//! Reading-stats transport. Wraps the per-user summary fetch and the two
//! library-scoped reads beside it for mobile (`GET /api/stats`,
//! `GET /api/library-size`, `GET /api/library-composition` via reqwest) and
//! web/SSR (the matching server functions). Both variants share their
//! signatures so the stats page stays platform-agnostic; the `#[cfg]` gates
//! carry the split.

use omnibus_shared::{
    LibraryComposition, LibrarySize, ReadingGoal, ReadingGoalUpdate, StatsRange, StatsSummary,
};

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

/// PUT `/api/stats/goal` — set, change, or clear the annual reading goal.
///
/// Rule 08 test 1: a goal is account configuration, so this is a direct call
/// on both targets and never enters the offline outbox. Callers disable the
/// control while offline and surface the failure when it fires anyway.
#[cfg(feature = "mobile")]
pub async fn save_reading_goal(
    server_url: &str,
    update: &ReadingGoalUpdate,
) -> Result<Option<ReadingGoal>, DataError> {
    let url = format!("{server_url}/api/stats/goal");
    let response = with_bearer(http_client().put(&url))
        .json(update)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Option<ReadingGoal>>().await?)
}

/// Web/SSR `save_reading_goal` — proxies to `rpc_set_reading_goal`.
#[cfg(not(feature = "mobile"))]
pub async fn save_reading_goal(
    _server_url: &str,
    update: &ReadingGoalUpdate,
) -> Result<Option<ReadingGoal>, DataError> {
    crate::rpc::rpc_set_reading_goal(update.clone())
        .await
        .map_err(note_server_fn_err)
}

/// GET `/api/library-composition` — what the library is made of. Not per-user
/// and not windowed, so it carries no query.
#[cfg(feature = "mobile")]
pub async fn fetch_library_composition(server_url: &str) -> Result<LibraryComposition, DataError> {
    let url = server_url.to_string();
    crate::offline::cache::read_through(
        crate::offline::cache::keys::library_composition(),
        async move {
            let response = with_bearer(http_client().get(format!("{url}/api/library-composition")))
                .send()
                .await?;
            let status = note_status(response.status());
            if !status.is_success() {
                return Err(drain_error(response, status).await);
            }
            Ok(response.json::<LibraryComposition>().await?)
        },
    )
    .await
}

/// Web/SSR `fetch_library_composition` — proxies to `rpc_library_composition`.
#[cfg(not(feature = "mobile"))]
pub async fn fetch_library_composition(_server_url: &str) -> Result<LibraryComposition, DataError> {
    crate::rpc::rpc_library_composition()
        .await
        .map_err(note_server_fn_err)
}
