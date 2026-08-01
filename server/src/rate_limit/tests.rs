//! Tests for the per-IP rate limiter: window/max enforcement, per-IP bucket
//! isolation, window reset, prefix-scoped middleware pass-through, the
//! shared-budget wiring between REST and RPC search, the auth-router
//! allow-list, and bucket pruning at capacity.

use super::*;

#[tokio::test]
async fn rate_limiter_allow_up_to_max_then_blocks() {
    let rl = RateLimiter::with_policy(Duration::from_secs(60), 3);
    let ip: IpAddr = "127.0.0.1".parse().unwrap();
    assert!(rl.allow(ip).await);
    assert!(rl.allow(ip).await);
    assert!(rl.allow(ip).await);
    assert!(!rl.allow(ip).await);
}

#[tokio::test]
async fn rate_limiter_allow_separate_ips_have_separate_buckets() {
    let rl = RateLimiter::with_policy(Duration::from_secs(60), 1);
    let a: IpAddr = "127.0.0.1".parse().unwrap();
    let b: IpAddr = "127.0.0.2".parse().unwrap();
    assert!(rl.allow(a).await);
    assert!(!rl.allow(a).await);
    assert!(rl.allow(b).await);
}

#[tokio::test]
async fn rate_limiter_allow_window_resets_after_elapsed() {
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
    // #249: REST and RPC search layers handed the same Arc must share one
    // per-IP budget. `oneshot` has no ConnectInfo, so all share 0.0.0.0.
    use axum::middleware::from_fn_with_state;
    use axum::{body::Body, routing::get, Router};
    use tower::ServiceExt;

    let max = 4u32;
    let limiter = Arc::new(RateLimiter::with_policy(Duration::from_secs(60), max));
    // Prefix covers both /api/rpc/search and /api/rpc/search-palette.
    let rpc_prefixes: Arc<Vec<&'static str>> = Arc::new(vec!["/api/rpc/search"]);

    // REST limited by rate_limit_by_ip, RPC by rate_limit_paths — same Arc.
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
    // Both RPC search routes are now exhausted too, proving one shared
    // budget — and that the full search is covered, not just the palette.
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
async fn rate_limiter_allow_prunes_stale_entries_at_cap() {
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
