//! Prometheus HTTP metrics: the middleware layer plus the `/metrics` scrape
//! route. Wired into the app router in `main.rs`; read by an external
//! Prometheus scraper.

use std::sync::OnceLock;

use axum::{routing::get, Router};
use axum_prometheus::PrometheusMetricLayer;

/// Build (once) the Prometheus metrics middleware and its `/metrics` scrape
/// route, returning clones.
///
/// `PrometheusMetricLayer::pair()` installs the process-global metrics recorder
/// and panics on a second call, so the (layer, route) is memoized in a
/// `OnceLock` and every call after the first returns clones of it — the
/// recorder is installed exactly once no matter how many times this runs. Apply
/// the returned layer as the outermost app layer so it observes every request,
/// and merge the returned router into the app. Series are labeled by method,
/// status, and `MatchedPath`, so id-bearing routes collapse to their registered
/// pattern (`/api/ebooks/{uuid}`) and label cardinality stays bounded.
/// `/metrics` sits outside `/api/`, so `auth::require_auth` passes it through
/// unauthenticated for the scraper.
pub fn layer_and_route() -> (PrometheusMetricLayer<'static>, Router) {
    static CELL: OnceLock<(PrometheusMetricLayer<'static>, Router)> = OnceLock::new();
    CELL.get_or_init(|| {
        let (layer, handle) = PrometheusMetricLayer::pair();
        let route = Router::new().route(
            "/metrics",
            get(move || {
                let handle = handle.clone();
                async move { handle.render() }
            }),
        );
        (layer, route)
    })
    .clone()
}

#[cfg(test)]
mod tests;
