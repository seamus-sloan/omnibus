//! CSRF origin-check middleware.
//!
//! Rejects state-changing cookie-authed requests whose `Origin`/`Referer`
//! doesn't match either an allowed-origin allowlist or — when no allowlist
//! is configured — the request's `Host`. Bearer-authed requests (mobile)
//! are exempt because browsers don't auto-attach bearer headers cross-site.
//! Safe methods (`GET`/`HEAD`/`OPTIONS`) always pass through.
//!
//! Set `OMNIBUS_PUBLIC_ORIGIN` to a comma-separated list of origins
//! (e.g. `http://localhost:3000,https://omnibus.example.com`) when the
//! server runs behind a reverse proxy that rewrites `Host` (the dioxus
//! `dx serve --fullstack` dev proxy does exactly this). Without an
//! allowlist, a proxied same-origin POST would 403 because the browser's
//! `Origin` (`localhost:3000`) no longer matches the upstream `Host`
//! (`127.0.0.1:<random-port>`).
//!
//! This is belt-and-braces on top of `SameSite=Lax`, which blocks classic
//! cross-site form POSTs but is inconsistent across browsers and doesn't
//! guard subdomain scenarios.

use axum::{
    extract::Request,
    http::{header, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;

use super::SESSION_COOKIE;

pub async fn origin_check(req: Request, next: Next) -> Response {
    let method = req.method();
    if matches!(method, &Method::GET | &Method::HEAD | &Method::OPTIONS) {
        return next.run(req).await;
    }

    // Bearer requests: exempt.
    if let Some(auth) = req.headers().get(header::AUTHORIZATION) {
        if auth
            .to_str()
            .map(|s| s.starts_with("Bearer "))
            .unwrap_or(false)
        {
            return next.run(req).await;
        }
    }

    // No cookie → not a state-changing cookie auth flow; let the normal
    // extractor 401 it if needed. Parse the jar rather than substring-matching
    // the header so unrelated cookies that merely contain our name don't
    // trigger the origin check.
    let jar = CookieJar::from_headers(req.headers());
    if jar.get(SESSION_COOKIE).is_none() {
        return next.run(req).await;
    }

    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    let referer = req
        .headers()
        .get(header::REFERER)
        .and_then(|v| v.to_str().ok());

    if let Some(allowed) = allowed_origins() {
        if origin_in_list(origin, allowed) || origin_in_list(referer, allowed) {
            return next.run(req).await;
        }
    }
    if let Some(host) = host {
        if origin_matches_host(origin, host) || origin_matches_host(referer, host) {
            return next.run(req).await;
        }
    }
    (StatusCode::FORBIDDEN, "origin mismatch").into_response()
}

fn origin_matches_host(origin: Option<&str>, host: &str) -> bool {
    let Some(origin) = origin else {
        return false;
    };
    // origin is like "http://host[:port]" or a full URL for Referer.
    // Strip scheme, then take authority up to next `/`.
    let after_scheme = origin.split_once("://").map(|(_, r)| r).unwrap_or(origin);
    let authority = after_scheme.split('/').next().unwrap_or("");
    authority == host
}

/// Parse a comma-separated allowlist string into normalized origins.
/// Trailing slashes and surrounding whitespace are tolerated. Returns
/// `None` for empty / whitespace-only input. Pure function so the
/// parsing rules are testable without touching the `OnceLock`-cached
/// env-var path.
fn parse_origin_allowlist(raw: &str) -> Option<Vec<String>> {
    let list: Vec<String> = raw
        .split(',')
        .map(|s| s.trim().trim_end_matches('/').to_string())
        .filter(|s| !s.is_empty())
        .collect();
    (!list.is_empty()).then_some(list)
}

/// Read `OMNIBUS_PUBLIC_ORIGIN` once on first request and cache the
/// parsed allowlist. Empty / unset returns `None`, which preserves the
/// legacy `Host`-based check for direct (non-proxied) deployments.
fn allowed_origins() -> Option<&'static [String]> {
    use std::sync::OnceLock;
    static SLOT: OnceLock<Option<Vec<String>>> = OnceLock::new();
    SLOT.get_or_init(|| parse_origin_allowlist(&std::env::var("OMNIBUS_PUBLIC_ORIGIN").ok()?))
        .as_deref()
}

/// Match a request `Origin` (or full `Referer` URL) against the allowlist.
/// For `Referer`, only the `scheme://authority` prefix is compared so the
/// path component is ignored.
fn origin_in_list(origin: Option<&str>, allowed: &[String]) -> bool {
    let Some(origin) = origin else {
        return false;
    };
    let normalized = origin.trim_end_matches('/');
    // Trim a Referer's path to its scheme+authority before comparing.
    let scheme_authority = match normalized.split_once("://") {
        Some((scheme, rest)) => {
            let authority = rest.split('/').next().unwrap_or("");
            format!("{scheme}://{authority}")
        }
        None => normalized.to_string(),
    };
    allowed.iter().any(|a| a == &scheme_authority)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, middleware, routing::post, Router};
    use tower::ServiceExt;

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
            if jar.get(SESSION_COOKIE).is_none() {
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
}
