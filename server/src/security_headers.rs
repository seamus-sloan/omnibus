//! Global HTTP security response headers (#277).
//!
//! Applies a single conservative set of headers to every response served by
//! the axum router — covers SSR HTML, the Dioxus WASM bundle, server-function
//! responses, the hand-written `/api/*` REST surface, and static assets in
//! one place rather than depending on each handler to remember.
//!
//! Header rationale:
//!
//! * **`Content-Security-Policy`** — defense-in-depth against any XSS gap in
//!   the user-supplied content rendered through SSR (book descriptions are
//!   sanitized via `ammonia`, but the policy keeps an injection limited to
//!   the page's own origin). The policy is intentionally generous enough to
//!   hydrate Dioxus today (`'wasm-unsafe-eval'` on `script-src` because
//!   `WebAssembly.instantiate` is treated as `eval` by the browser CSP
//!   engine; `'unsafe-inline'` on `style-src` because Dioxus emits inline
//!   `style=""` attributes; `data:` / `blob:` on `img-src` to cover thumbs
//!   and base64-embedded images). Tighten incrementally as the asset surface
//!   stabilizes.
//! * **`X-Frame-Options: DENY`** — legacy clickjacking guard. `frame-ancestors
//!   'none'` in the CSP supersedes this on modern browsers; both ship for
//!   coverage on older clients.
//! * **`Referrer-Policy: strict-origin-when-cross-origin`** — keeps full URLs
//!   (which may include search queries or book ids) from leaking to other
//!   origins, while preserving same-origin referrers used for analytics on
//!   internal navigation.
//! * **`X-Content-Type-Options: nosniff`** — globally suppresses MIME
//!   sniffing. The per-handler `nosniff` on cover/thumb responses in
//!   `backend::covers` becomes redundant once this layer is mounted, but
//!   stays as belt-and-suspenders.
//! * **`Strict-Transport-Security`** — only emitted when
//!   `OMNIBUS_SECURE_COOKIES` resolves to truthy via the shared
//!   `auth::handlers::parse_secure_cookies` reader. Sharing the parse
//!   keeps the HSTS toggle pinned to the cookie `Secure` toggle so the
//!   two can't drift; sending HSTS over plain HTTP is ignored by browsers
//!   but would advertise the wrong policy on a LAN-IP dev origin where
//!   the operator has set `OMNIBUS_SECURE_COOKIES=0`.

use axum::http::{header, HeaderName, HeaderValue};
use tower_http::set_header::SetResponseHeaderLayer;

/// Conservative content-security policy that still hydrates Dioxus WASM.
/// See module docs for the per-directive rationale.
const DEFAULT_CSP: &str = "default-src 'self'; \
script-src 'self' 'wasm-unsafe-eval'; \
style-src 'self' 'unsafe-inline'; \
img-src 'self' data: blob:; \
font-src 'self' data:; \
connect-src 'self'; \
object-src 'none'; \
base-uri 'self'; \
form-action 'self'; \
frame-ancestors 'none'";

/// One year, `includeSubDomains`. Standard production HSTS recommendation;
/// preload is intentionally omitted (operator opt-in only).
const DEFAULT_HSTS: &str = "max-age=31536000; includeSubDomains";

/// Build the `(CSP, X-Frame-Options, Referrer-Policy, X-Content-Type-Options)`
/// layers applied unconditionally to every response.
///
/// Returned as an array of layers so the caller can fold each via repeated
/// `.layer(...)` calls without us needing to build a custom `tower::Layer`
/// composition (axum's `Router` only stacks layers one at a time).
pub fn baseline_layers() -> [SetResponseHeaderLayer<HeaderValue>; 4] {
    [
        SetResponseHeaderLayer::overriding(
            header::CONTENT_SECURITY_POLICY,
            HeaderValue::from_static(DEFAULT_CSP),
        ),
        SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ),
        SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ),
        SetResponseHeaderLayer::overriding(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ),
    ]
}

/// HSTS layer, conditionally constructed.
///
/// Returns `Some(layer)` when `secure_cookies` is true (the operator has
/// opted into HTTPS-only cookies), `None` otherwise. The caller resolves
/// the toggle by passing the result of `auth::handlers::parse_secure_cookies`
/// — sharing the single parser keeps HSTS pinned to the cookie `Secure`
/// flag so the two policies can't drift.
pub fn hsts_layer(secure_cookies: bool) -> Option<SetResponseHeaderLayer<HeaderValue>> {
    secure_cookies.then(|| {
        SetResponseHeaderLayer::overriding(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(DEFAULT_HSTS),
        )
    })
}

#[cfg(test)]
mod tests {
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
            Some(DEFAULT_CSP)
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
    async fn csp_permits_wasm_eval_and_inline_styles() {
        // Guard rails so a future tightening of the CSP doesn't silently
        // break Dioxus hydration in the browser. WebAssembly.instantiate is
        // treated as `eval` by the CSP engine; Dioxus emits inline style
        // attributes; thumbnails are served as data:/blob: URLs in some
        // code paths.
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
            csp.contains("'wasm-unsafe-eval'"),
            "csp missing wasm: {csp}"
        );
        assert!(
            csp.contains("style-src 'self' 'unsafe-inline'"),
            "csp must allow inline styles: {csp}"
        );
        assert!(
            csp.contains("img-src 'self' data: blob:"),
            "csp must allow data: + blob: images: {csp}"
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
        assert_eq!(all[0].to_str().unwrap(), DEFAULT_CSP);
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
}
