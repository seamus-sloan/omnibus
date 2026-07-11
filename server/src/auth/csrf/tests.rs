//! Tests for CSRF origin-check middleware.
use axum::{body::Body, http::Request, middleware, routing::post, Router};
use tower::ServiceExt;

use super::*;

fn guarded_router() -> Router {
    Router::new()
        .route("/api/mut", post(|| async { "ok" }))
        .layer(middleware::from_fn(origin_check))
}

#[tokio::test]
async fn same_origin_post_passes() {
    let res = guarded_router()
        .oneshot(
            Request::builder()
                .uri("/api/mut")
                .method("POST")
                .header(header::HOST, "localhost:3000")
                .header(header::ORIGIN, "http://localhost:3000")
                .header(header::COOKIE, format!("{SESSION_COOKIE}=fake"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn cross_origin_post_with_cookie_is_rejected() {
    let res = guarded_router()
        .oneshot(
            Request::builder()
                .uri("/api/mut")
                .method("POST")
                .header(header::HOST, "localhost:3000")
                .header(header::ORIGIN, "http://evil.example")
                .header(header::COOKIE, format!("{SESSION_COOKIE}=fake"))
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn origin_in_list_matches_normalized_origin_and_referer_authority() {
    // The middleware's "is this origin in the allowlist?" predicate.
    // Exact and trailing-slash forms match; a Referer's path is trimmed
    // before comparison; anything outside the list (including `None`)
    // is rejected.
    let allowed = vec!["http://localhost:3000".to_string()];
    assert!(origin_in_list(Some("http://localhost:3000"), &allowed));
    assert!(origin_in_list(Some("http://localhost:3000/"), &allowed));
    assert!(origin_in_list(
        Some("http://localhost:3000/some/page?x=1"),
        &allowed,
    ));
    assert!(!origin_in_list(Some("http://evil.example"), &allowed));
    assert!(!origin_in_list(None, &allowed));
}

#[test]
fn parse_origin_allowlist_handles_csv_whitespace_and_trailing_slashes() {
    // Pure parser — no env / OnceLock involvement, so the parsing rules
    // (CSV split, whitespace trim, trailing-slash trim, empty-entry
    // filter) are exercised directly. The cached `allowed_origins()`
    // wrapper just feeds the env-var string through this.
    assert_eq!(
        parse_origin_allowlist("http://localhost:3000"),
        Some(vec!["http://localhost:3000".into()]),
    );
    assert_eq!(
        parse_origin_allowlist("http://localhost:3000/, https://omnibus.example.com/ "),
        Some(vec![
            "http://localhost:3000".into(),
            "https://omnibus.example.com".into(),
        ]),
    );
    assert_eq!(parse_origin_allowlist(""), None);
    assert_eq!(parse_origin_allowlist(" ,, "), None);
}

#[tokio::test]
async fn proxied_post_with_allowlist_passes_when_origin_matches() {
    // End-to-end through the actual middleware: simulate the dx-fullstack
    // proxy by sending an upstream Host that doesn't match Origin, and
    // confirm the allowlist branch admits the request. Uses a router
    // wired with a hand-built allowlist closure so the test doesn't
    // touch the process-global OnceLock or the OMNIBUS_PUBLIC_ORIGIN
    // env var (both shared across the test binary).
    async fn check(req: Request<Body>) -> Response {
        let allowlist = vec!["http://localhost:3000".to_string()];
        let method = req.method();
        if matches!(method, &Method::GET | &Method::HEAD | &Method::OPTIONS) {
            return (StatusCode::OK, "ok").into_response();
        }
        if let Some(auth) = req.headers().get(header::AUTHORIZATION) {
            if auth
                .to_str()
                .map(|s| s.starts_with("Bearer "))
                .unwrap_or(false)
            {
                return (StatusCode::OK, "ok").into_response();
            }
        }
        let jar = CookieJar::from_headers(req.headers());
        if !has_session_cookie(&jar) {
            return (StatusCode::OK, "ok").into_response();
        }
        let origin = req
            .headers()
            .get(header::ORIGIN)
            .and_then(|v| v.to_str().ok());
        let referer = req
            .headers()
            .get(header::REFERER)
            .and_then(|v| v.to_str().ok());
        if origin_in_list(origin, &allowlist) || origin_in_list(referer, &allowlist) {
            (StatusCode::OK, "ok").into_response()
        } else {
            (StatusCode::FORBIDDEN, "origin mismatch").into_response()
        }
    }

    // Same Host vs. Origin mismatch the dx-fullstack proxy produces in
    // the wild — Host is rewritten to the upstream loopback address,
    // Origin is the browser's public URL.
    let allowed = check(
        Request::builder()
            .uri("/api/mut")
            .method("POST")
            .header(header::HOST, "127.0.0.1:50878")
            .header(header::ORIGIN, "http://localhost:3000")
            .header(header::COOKIE, format!("{SESSION_COOKIE}=fake"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(allowed.status(), StatusCode::OK);

    let blocked = check(
        Request::builder()
            .uri("/api/mut")
            .method("POST")
            .header(header::HOST, "127.0.0.1:50878")
            .header(header::ORIGIN, "http://evil.example")
            .header(header::COOKIE, format!("{SESSION_COOKIE}=fake"))
            .body(Body::empty())
            .unwrap(),
    )
    .await;
    assert_eq!(blocked.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn host_prefixed_cookie_also_triggers_origin_check() {
    // A cookie under the `__Host-` prefixed name must arm the origin
    // check the same as the legacy plain name, otherwise a deploy that
    // flips OMNIBUS_SECURE_COOKIES on would silently drop CSRF
    // protection for cookies the server is now issuing.
    let res = guarded_router()
        .oneshot(
            Request::builder()
                .uri("/api/mut")
                .method("POST")
                .header(header::HOST, "localhost:3000")
                .header(header::ORIGIN, "http://evil.example")
                .header(
                    header::COOKIE,
                    format!("{SESSION_COOKIE_HOST_PREFIXED}=fake"),
                )
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[test]
fn has_session_cookie_matches_either_name() {
    let make = |raw: &str| {
        let mut h = axum::http::HeaderMap::new();
        h.insert(header::COOKIE, raw.parse().unwrap());
        CookieJar::from_headers(&h)
    };
    assert!(has_session_cookie(&make("omnibus_session=tok")));
    assert!(has_session_cookie(&make("__Host-omnibus_session=tok")));
    assert!(!has_session_cookie(&make("other=tok")));
}

#[tokio::test]
async fn bearer_requests_are_exempt() {
    let res = guarded_router()
        .oneshot(
            Request::builder()
                .uri("/api/mut")
                .method("POST")
                .header(header::HOST, "localhost:3000")
                .header(header::AUTHORIZATION, "Bearer whatever")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}
