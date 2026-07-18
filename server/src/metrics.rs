//! Prometheus HTTP metrics: the middleware layer plus the `/metrics` scrape
//! route. Wired into the app router in `main.rs`; read by an external
//! Prometheus scraper.

use axum::{routing::get, Router};
use axum_prometheus::PrometheusMetricLayer;

/// Build the Prometheus metrics middleware and its `/metrics` scrape route.
///
/// `PrometheusMetricLayer::pair()` installs the process-global metrics
/// recorder, so this must be called **exactly once** per process (a second
/// call panics). Apply the returned layer as the outermost app layer so it
/// observes every request, and merge the returned router into the app. Series
/// are labeled by method, status, and `MatchedPath`, so id-bearing routes
/// collapse to their registered pattern (`/api/ebooks/{uuid}`) and label
/// cardinality stays bounded. `/metrics` sits outside `/api/`, so
/// `auth::require_auth` passes it through unauthenticated for the scraper.
pub fn layer_and_route() -> (PrometheusMetricLayer<'static>, Router) {
    let (layer, handle) = PrometheusMetricLayer::pair();
    let route = Router::new().route(
        "/metrics",
        get(move || {
            let handle = handle.clone();
            async move { handle.render() }
        }),
    );
    (layer, route)
}

#[cfg(test)]
mod tests;
