//! Tests for the security-headers middleware: baseline headers present on
//! every response, CSP permissiveness for Dioxus hydration and fonts,
//! handler-supplied overrides winning over the baseline, and HSTS toggling.

use super::*;
use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    routing::get,
    Router,
};
use tower::ServiceExt;

fn app_with_baseline() -> Router {
    let mut router = Router::new().route("/", get(|| async { "ok" }));
    for layer in baseline_layers() {
        router = router.layer(layer);
    }
    router
}

#[tokio::test]
async fn baseline_headers_present_on_every_response() {
    let app = app_with_baseline();
    let res = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let headers = res.headers();
    assert_eq!(
        headers
            .get(header::CONTENT_SECURITY_POLICY)
            .and_then(|v| v.to_str().ok()),
        Some(DEFAULT_CSP.as_str())
    );
    assert_eq!(
        headers.get("x-frame-options").and_then(|v| v.to_str().ok()),
        Some("DENY")
    );
    assert_eq!(
        headers
            .get(header::REFERRER_POLICY)
            .and_then(|v| v.to_str().ok()),
        Some("strict-origin-when-cross-origin")
    );
    assert_eq!(
        headers
            .get(header::X_CONTENT_TYPE_OPTIONS)
            .and_then(|v| v.to_str().ok()),
        Some("nosniff")
    );
    let _ = to_bytes(res.into_body(), 1024).await.unwrap();
}

#[tokio::test]
async fn csp_permits_dioxus_hydration_and_fonts() {
    // Guard rails so a future tightening of the CSP doesn't silently
    // re-break the live page. These assertions encode hard requirements
    // discovered in-browser (the header-only checks below can't observe
    // them):
    //   * Dioxus fullstack emits its hydration bootstrap + serialized
    //     state as INLINE <script> tags with no nonce, so script-src must
    //     carry 'unsafe-inline' or the SSR markup never hydrates.
    //   * dioxus-interpreter-js builds functions via the Function()
    //     constructor → needs 'unsafe-eval' (NOT just 'wasm-unsafe-eval',
    //     which only permits WebAssembly.instantiate; the WASM panics on
    //     init under it). 'unsafe-eval' subsumes WASM instantiation.
    //   * atrium.css @imports Google Fonts (stylesheet on fonts.googleapis
    //     .com, WOFF2 glyphs on fonts.gstatic.com) → style-src + font-src
    //     must allowlist those hosts.
    //   * Dioxus emits inline style="" attributes → style-src 'unsafe-inline'.
    //   * thumbnails are served as data:/blob: URLs in some paths.
    let app = app_with_baseline();
    let res = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let csp = res
        .headers()
        .get(header::CONTENT_SECURITY_POLICY)
        .and_then(|v| v.to_str().ok())
        .unwrap();
    assert!(
        csp.contains("script-src 'self' 'unsafe-inline' 'unsafe-eval'"),
        "csp must allow inline (hydration) + Function()-eval (dioxus interpreter) scripts: {csp}"
    );
    assert!(
        csp.contains("style-src 'self' 'unsafe-inline' https://fonts.googleapis.com"),
        "csp must allow inline styles + Google Fonts stylesheet host: {csp}"
    );
    assert!(
        csp.contains("font-src 'self' data: https://fonts.gstatic.com"),
        "csp must allow the Google Fonts WOFF2 host: {csp}"
    );
    assert!(
        csp.contains("img-src 'self' data: blob: https://"),
        "csp must allow data:/blob: images plus provider cover hosts: {csp}"
    );
    // Derived from the catalog, so the assertion is too: a provider added
    // without its cover host reaching the CSP fails here rather than in a
    // browser console nobody is watching.
    for host in omnibus_db::all_cover_hosts() {
        assert!(
            csp.contains(&format!("https://{host}")),
            "csp img-src must name the provider cover host {host}: {csp}"
        );
    }
    // The redirect target Open Library's cover CDN 302s to, which browsers
    // check against `img-src` as well as the original request.
    assert!(
        csp.contains("https://archive.org"),
        "csp must allow Open Library's cover redirect target: {csp}"
    );
    assert!(
        csp.contains("frame-ancestors 'none'"),
        "csp must deny framing: {csp}"
    );
}

#[tokio::test]
async fn baseline_layer_overrides_handler_supplied_value() {
    // The global layer is wired with `overriding`, so a stale per-handler
    // value (e.g. a weaker policy left over from before this layer was
    // added) is replaced rather than appended-to. This keeps the headers
    // canonical: every response carries exactly one CSP / X-Frame /
    // Referrer-Policy / nosniff, not several comma-joined.
    async fn weak() -> impl axum::response::IntoResponse {
        ([(header::CONTENT_SECURITY_POLICY, "default-src *")], "ok")
    }
    let mut router = Router::new().route("/", get(weak));
    for layer in baseline_layers() {
        router = router.layer(layer);
    }
    let res = router
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    let all: Vec<_> = res
        .headers()
        .get_all(header::CONTENT_SECURITY_POLICY)
        .iter()
        .collect();
    assert_eq!(all.len(), 1, "expected exactly one CSP header, got {all:?}");
    assert_eq!(all[0].to_str().unwrap(), DEFAULT_CSP.as_str());
}

#[tokio::test]
async fn hsts_layer_when_enabled_emits_one_year_include_subdomains() {
    let layer = hsts_layer(true).expect("hsts_layer(true) must yield a layer");
    let app = Router::new()
        .route("/", get(|| async { "ok" }))
        .layer(layer);
    let res = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();
    assert_eq!(
        res.headers()
            .get(header::STRICT_TRANSPORT_SECURITY)
            .and_then(|v| v.to_str().ok()),
        Some(DEFAULT_HSTS)
    );
}

#[test]
fn hsts_layer_when_disabled_returns_none() {
    assert!(hsts_layer(false).is_none());
}

#[tokio::test]
async fn the_provider_host_fallback_policy_is_strictly_stricter_than_the_default() {
    // The fallback exists for a header value the catalog could never produce
    // today, so nothing exercises it in production — which is exactly why it
    // has to be checked here. Dropping to a shorter policy would give up the
    // framing and base-uri guards along with the image hosts.
    for directive in [
        "default-src 'self'",
        "object-src 'none'",
        "base-uri 'self'",
        "form-action 'self'",
        "frame-ancestors 'none'",
        "connect-src 'self'",
    ] {
        assert!(
            NO_PROVIDER_HOSTS_CSP.contains(directive),
            "fallback policy must keep {directive}: {NO_PROVIDER_HOSTS_CSP}"
        );
    }
    // And it must name no provider host at all — that is the whole point of
    // falling back to it.
    assert!(NO_PROVIDER_HOSTS_CSP.contains("img-src 'self' data: blob:;"));
    for host in omnibus_db::all_cover_hosts() {
        assert!(
            !NO_PROVIDER_HOSTS_CSP.contains(host),
            "fallback policy must not name {host}"
        );
    }
}
