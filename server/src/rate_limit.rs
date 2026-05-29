//! Reusable in-memory per-IP rate limiter and axum middleware.
//!
//! Self-hosted scope is small — a single process, ≤ a few dozen users. A
//! Redis-backed limiter is overkill; a `Mutex<HashMap<IpAddr, Bucket>>` fits
//! in < 60 lines and is easy to reason about. Swap for `tower_governor` if
//! the scope ever grows.
//!
//! Policy: fixed-window counter, `max` requests per `window` per principal.
//! The principal is the request's peer IP (via `ConnectInfo<SocketAddr>`),
//! with an optional opt-in `X-Forwarded-For` fallback when
//! `OMNIBUS_TRUST_FORWARDED_FOR=1`.
//!
//! Mount via `axum::middleware::from_fn_with_state(limiter, rate_limit_by_ip)`
//! on whichever sub-router needs limiting. The middleware itself does **not**
//! filter on path — restriction happens at the router level, so the same
//! primitive serves `/api/auth/{login,register}`, `/api/search/*`, and any
//! future route that wants the same treatment without copy-paste.
//!
//! Default policy (`RateLimiter::new`) is tuned for the auth endpoints:
//! 10 requests / 60s / IP. Search uses a separate limiter built via
//! [`RateLimiter::with_policy`] with a tighter budget (30 / 10s).
//!
//! The auth limiter is mounted via [`rate_limit_paths`] with a prefix
//! allow-list of `/api/auth/{login,register,logout}` — `/api/auth/me` is
//! deliberately exempt. `/me` is an authenticated read of the caller's
//! own row and presents no brute-force surface, so sharing the 10/60s
//! bucket with credential endpoints just throttled legitimate UI boots
//! (and parallel Playwright workers from the loopback IP).

use axum::{
    extract::{ConnectInfo, Request, State},
    http::StatusCode,
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

/// Default window for the auth-endpoint limiter.
pub const WINDOW_SECS: u64 = 60;
/// Default max requests per window for the auth-endpoint limiter.
pub const MAX_REQUESTS: u32 = 10;
/// Maximum number of tracked IPs before stale buckets are pruned.
const MAX_BUCKETS: usize = 10_000;

fn trust_forwarded_for() -> bool {
    matches!(
        std::env::var("OMNIBUS_TRUST_FORWARDED_FOR").as_deref(),
        Ok("1" | "true" | "yes")
    )
}

struct Bucket {
    window_start: Instant,
    count: u32,
}

/// `tokio::sync::Mutex` is used here rather than `std::sync::Mutex` so that
/// `allow()` never blocks a Tokio worker thread while waiting for the lock
/// under contention.
pub struct RateLimiter {
    inner: Mutex<HashMap<IpAddr, Bucket>>,
    window: Duration,
    max: u32,
}

impl RateLimiter {
    /// Default policy: `MAX_REQUESTS` per `WINDOW_SECS`.
    pub fn new() -> Self {
        Self::with_policy(Duration::from_secs(WINDOW_SECS), MAX_REQUESTS)
    }

    pub fn with_policy(window: Duration, max: u32) -> Self {
        Self {
            inner: Mutex::new(HashMap::new()),
            window,
            max,
        }
    }

    pub async fn allow(&self, ip: IpAddr) -> bool {
        let now = Instant::now();
        let mut map = self.inner.lock().await;

        // Prune stale entries when the map gets large to prevent unbounded growth.
        if map.len() >= MAX_BUCKETS {
            let window = self.window;
            map.retain(|_, b| now.duration_since(b.window_start) < window * 2);
        }

        let bucket = map.entry(ip).or_insert(Bucket {
            window_start: now,
            count: 0,
        });
        if now.duration_since(bucket.window_start) >= self.window {
            bucket.window_start = now;
            bucket.count = 0;
        }
        if bucket.count >= self.max {
            return false;
        }
        bucket.count += 1;
        true
    }
}

impl Default for RateLimiter {
    fn default() -> Self {
        Self::new()
    }
}

/// Resolve the request principal IP. Prefers `ConnectInfo<SocketAddr>` (wired
/// by the server's make-service). Only consults `X-Forwarded-For` when the
/// operator has opted in via `OMNIBUS_TRUST_FORWARDED_FOR=1` — otherwise a
/// client on a directly-reachable deployment could spoof the header to
/// bypass the limiter and grow the bucket map without bound. When neither
/// source yields an IP, falls back to `0.0.0.0` so the limiter still applies
/// process-wide.
fn resolve_ip(req: &Request) -> IpAddr {
    let direct = req
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|ConnectInfo(a)| a.ip());
    direct
        .or_else(|| {
            if !trust_forwarded_for() {
                return None;
            }
            req.headers()
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.split(',').next())
                .and_then(|s| s.trim().parse().ok())
        })
        .unwrap_or(IpAddr::V4(Ipv4Addr::UNSPECIFIED))
}

/// Generic per-IP rate-limit middleware. Apply to whichever sub-router needs
/// limiting — the middleware does no path filtering of its own. Returns
/// `429 Too Many Requests` once the limiter's policy is exceeded; otherwise
/// passes the request through.
pub async fn rate_limit_by_ip(
    State(limiter): State<Arc<RateLimiter>>,
    req: Request,
    next: Next,
) -> Response {
    let ip = resolve_ip(&req);
    if !limiter.allow(ip).await {
        return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
    }
    next.run(req).await
}

/// Path-prefix wrapper around [`rate_limit_by_ip`]. The middleware ignores
/// (passes through) any request whose path doesn't match one of the given
/// prefixes — handy when a top-level router contains a mix of routes and
/// only a subset should be limited (e.g. the dioxus fullstack RPC router,
/// where we only want to throttle `/api/rpc/search-*`).
///
/// Bucket map is shared with whatever limiter is passed in. `main.rs` passes
/// the *same* `Arc<RateLimiter>` here (for `/api/rpc/search-*`) and into the
/// REST `/api/search/*` layer (via [`rate_limit_by_ip`], threaded through
/// `backend::rest_router_with_search_limiter`), so both search families draw
/// from a single per-IP budget (#249).
pub async fn rate_limit_paths(
    State((limiter, prefixes)): State<(Arc<RateLimiter>, Arc<Vec<&'static str>>)>,
    req: Request,
    next: Next,
) -> Response {
    let path = req.uri().path();
    if !prefixes.iter().any(|p| path.starts_with(p)) {
        return next.run(req).await;
    }
    let ip = resolve_ip(&req);
    if !limiter.allow(ip).await {
        return (StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response();
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn allows_up_to_max_then_blocks() {
        let rl = RateLimiter::with_policy(Duration::from_secs(60), 3);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(rl.allow(ip).await);
        assert!(rl.allow(ip).await);
        assert!(rl.allow(ip).await);
        assert!(!rl.allow(ip).await);
    }

    #[tokio::test]
    async fn separate_ips_have_separate_buckets() {
        let rl = RateLimiter::with_policy(Duration::from_secs(60), 1);
        let a: IpAddr = "127.0.0.1".parse().unwrap();
        let b: IpAddr = "127.0.0.2".parse().unwrap();
        assert!(rl.allow(a).await);
        assert!(!rl.allow(a).await);
        assert!(rl.allow(b).await);
    }

    #[tokio::test]
    async fn window_resets_after_elapsed() {
        let rl = RateLimiter::with_policy(Duration::from_millis(10), 1);
        let ip: IpAddr = "127.0.0.1".parse().unwrap();
        assert!(rl.allow(ip).await);
        assert!(!rl.allow(ip).await);
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert!(rl.allow(ip).await);
    }

    #[tokio::test]
    async fn rate_limit_paths_limits_matching_prefix_and_passes_others() {
        // Mirrors the `main.rs` RPC wiring: `rate_limit_paths` mounted with
        // the search-palette prefix. Drives the route via `oneshot` to assert
        // both the over-limit (429) and the pass-through (non-matching path)
        // cases. `oneshot` carries no `ConnectInfo`, so every request shares
        // the `0.0.0.0` fallback bucket — exactly one budget under test.
        use axum::middleware::from_fn_with_state;
        use axum::{body::Body, routing::get, Router};
        use tower::ServiceExt;

        let max = 3u32;
        let limiter = Arc::new(RateLimiter::with_policy(Duration::from_secs(60), max));
        let prefixes: Arc<Vec<&'static str>> = Arc::new(vec!["/api/rpc/search-palette"]);
        let app = Router::new()
            .route("/api/rpc/search-palette", get(|| async { "ok" }))
            .route("/api/rpc/other", get(|| async { "ok" }))
            .layer(from_fn_with_state((limiter, prefixes), rate_limit_paths));

        let palette_req = || {
            Request::builder()
                .uri("/api/rpc/search-palette?q=hello")
                .body(Body::empty())
                .unwrap()
        };

        // Matching prefix: first `max` requests pass, the next trips the limiter.
        for i in 0..max {
            let res = app.clone().oneshot(palette_req()).await.unwrap();
            assert_eq!(res.status(), StatusCode::OK, "request #{i} within budget");
        }
        let over = app.clone().oneshot(palette_req()).await.unwrap();
        assert_eq!(over.status(), StatusCode::TOO_MANY_REQUESTS);

        // Non-matching `/api/rpc/*` path bypasses the limiter entirely: it still
        // returns 200 even though the shared bucket is already over budget,
        // proving the prefix filter short-circuits before `allow()`.
        for _ in 0..(max + 5) {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/rpc/other")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                res.status(),
                StatusCode::OK,
                "non-matching /api/rpc/* path must bypass the limiter"
            );
        }
    }

    #[tokio::test]
    async fn one_shared_limiter_unifies_rest_and_rpc_search_budget() {
        // #249: the REST `/api/search/*` layer (rate_limit_by_ip) and the RPC
        // `/api/rpc/search-*` layer (rate_limit_paths) must draw from ONE
        // per-IP budget when handed the same Arc — so spending the budget on
        // one family also blocks the other. `oneshot` carries no ConnectInfo,
        // so every request shares the 0.0.0.0 fallback bucket.
        use axum::middleware::from_fn_with_state;
        use axum::{body::Body, routing::get, Router};
        use tower::ServiceExt;

        let max = 4u32;
        let limiter = Arc::new(RateLimiter::with_policy(Duration::from_secs(60), max));
        // Prefix covers both /api/rpc/search and /api/rpc/search-palette.
        let rpc_prefixes: Arc<Vec<&'static str>> = Arc::new(vec!["/api/rpc/search"]);

        // REST sub-router limited by rate_limit_by_ip with the shared limiter;
        // the whole app's RPC path limited by rate_limit_paths with the SAME one.
        let rest = Router::new()
            .route("/api/search", get(|| async { "ok" }))
            .layer(from_fn_with_state(limiter.clone(), rate_limit_by_ip));
        let app = Router::new()
            .route("/api/rpc/search", get(|| async { "ok" }))
            .route("/api/rpc/search-palette", get(|| async { "ok" }))
            .merge(rest)
            .layer(from_fn_with_state(
                (limiter.clone(), rpc_prefixes),
                rate_limit_paths,
            ));

        // Spend the entire budget on the REST family.
        for i in 0..max {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/search?q=x")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                res.status(),
                StatusCode::OK,
                "REST request #{i} within budget"
            );
        }
        // Both RPC search routes are now also exhausted — proving a single
        // shared budget rather than the previous 2× (one per family). The full
        // search `/api/rpc/search` must be covered too, not just the palette
        // (#249): a narrower prefix would have left it as a bypass.
        for uri in ["/api/rpc/search?q=x", "/api/rpc/search-palette?q=x"] {
            let rpc = app
                .clone()
                .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
                .await
                .unwrap();
            assert_eq!(
                rpc.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "{uri} must be blocked once REST has spent the shared budget"
            );
        }
    }

    #[tokio::test]
    async fn auth_limiter_throttles_login_but_not_me() {
        // Mirrors the `main.rs` auth-router wiring: a default RateLimiter
        // mounted on the auth router via `rate_limit_paths` with a prefix
        // allow-list that *excludes* `/api/auth/me`. Verifies that bursting
        // /me past the 10/60s default budget never trips the limiter, while
        // /login on the same IP still does. Guards against future regression
        // where someone re-mounts the auth router with `rate_limit_by_ip`
        // (which would re-include /me in the bucket).
        use axum::middleware::from_fn_with_state;
        use axum::{body::Body, routing::get, routing::post, Router};
        use tower::ServiceExt;

        let limiter = Arc::new(RateLimiter::new()); // default 10/60s
        let prefixes: Arc<Vec<&'static str>> = Arc::new(vec![
            "/api/auth/login",
            "/api/auth/register",
            "/api/auth/logout",
        ]);
        let app = Router::new()
            .route("/api/auth/me", get(|| async { "ok" }))
            .route("/api/auth/login", post(|| async { "ok" }))
            .layer(from_fn_with_state((limiter, prefixes), rate_limit_paths));

        // Burst /me far past the 10/60s budget — every request must pass.
        for i in 0..(MAX_REQUESTS as usize * 3) {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/auth/me")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                res.status(),
                StatusCode::OK,
                "/api/auth/me must bypass the auth limiter (request #{i})"
            );
        }

        // /login on the same shared bucket (oneshot has no ConnectInfo so
        // everything resolves to the 0.0.0.0 fallback) must still throttle
        // after MAX_REQUESTS hits. The /me burst above did NOT consume from
        // the bucket, so the first MAX_REQUESTS logins all succeed.
        for i in 0..MAX_REQUESTS {
            let res = app
                .clone()
                .oneshot(
                    Request::builder()
                        .uri("/api/auth/login")
                        .method("POST")
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(
                res.status(),
                StatusCode::OK,
                "/api/auth/login within budget (request #{i})"
            );
        }
        let over = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/auth/login")
                    .method("POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            over.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "/api/auth/login must still be limited"
        );
    }

    #[tokio::test]
    async fn prunes_stale_entries_at_cap() {
        let rl = RateLimiter::with_policy(Duration::from_millis(1), MAX_REQUESTS);
        // Fill to just under the cap using distinct IPs.
        for i in 0..MAX_BUCKETS {
            let ip = IpAddr::V4(std::net::Ipv4Addr::from(i as u32));
            rl.allow(ip).await;
        }
        // All windows are stale; a new allow() call should prune and succeed.
        tokio::time::sleep(Duration::from_millis(10)).await;
        let ip: IpAddr = "1.2.3.4".parse().unwrap();
        assert!(rl.allow(ip).await);
        assert!(rl.inner.lock().await.len() < MAX_BUCKETS);
    }
}
