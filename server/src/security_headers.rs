//! Global HTTP security response headers, applied to every response served
//! by the axum router (SSR HTML, WASM bundle, server-function responses,
//! the `/api/*` REST surface, static assets) in one place rather than
//! depending on each handler to remember. See [`DEFAULT_CSP`],
//! [`baseline_layers`], and [`hsts_layer`] for the per-header rationale.

use axum::http::{header, HeaderName, HeaderValue};
use tower_http::set_header::SetResponseHeaderLayer;

/// Conservative content-security policy that still hydrates Dioxus WASM.
///
/// Defense-in-depth against any XSS gap in the user-supplied content
/// rendered through SSR (book descriptions are sanitized via `ammonia`, but
/// the policy keeps an injection limited to the page's own origin).
/// Per-directive rationale:
///
/// - `script-src 'self' 'unsafe-inline' 'unsafe-eval'` — three relaxations,
///   all forced by the Dioxus web runtime; none can be tightened without
///   upstream changes. `'self'` loads the external WASM glue
///   (`/wasm/omnibus.js`). `'unsafe-inline'` is required because Dioxus
///   fullstack emits its hydration bootstrap (`window.dx_hydrate = …`) and
///   serialized state (`window.initial_dioxus_hydration_data = …`) as inline
///   `<script>` tags with no nonce and a per-page-variable body, so the SSR
///   markup cannot hydrate without it (hashes are unstable across pages;
///   Dioxus stamps no nonce). `'unsafe-eval'` is required because the
///   `dioxus-interpreter-js` runtime builds functions at runtime via the
///   `Function()` constructor — classic `eval`; `'wasm-unsafe-eval'` only
///   permits `WebAssembly.instantiate`, NOT `Function()`/`eval()`, so the
///   WASM panics on init under it. `'unsafe-eval'` is a superset that also
///   covers WASM instantiation, so `'wasm-unsafe-eval'` is not listed
///   separately. Net effect: `script-src` provides essentially no
///   script-injection protection here — an inherent cost of Dioxus web
///   today. The CSP's real value lives in the *other* directives:
///   `connect-src 'self'` blocks exfiltration to foreign origins, `'self'`
///   still blocks loading *external* scripts, and `object-src` / `base-uri`
///   / `form-action` / `frame-ancestors` are all locked down. Combined with
///   `ammonia` sanitizing the primary XSS source (book descriptions), an
///   injected script's blast radius stays bounded. Revisit only if Dioxus
///   drops its `Function()` use and gains nonce support.
/// - `style-src 'self' 'unsafe-inline' https://fonts.googleapis.com` —
///   `'unsafe-inline'` because Dioxus emits inline `style=""` attributes;
///   the Google Fonts host because `atrium.css` `@import`s the Geist /
///   Instrument Serif stylesheet from it. Self-hosting the fonts (a
///   follow-up tracked in `atrium.css`) would let both the CDN host here
///   and in `font-src` drop back to `'self'`.
/// - `font-src 'self' data: https://fonts.gstatic.com` — Google serves the
///   actual WOFF2 files from `fonts.gstatic.com`; without it the `@import`ed
///   stylesheet resolves but the glyphs fall back to system fonts.
/// - `img-src 'self' data: blob:` — `data:` / `blob:` cover thumbnails and
///   base64-embedded images.
///
/// Tighten incrementally as the asset surface stabilizes.
const DEFAULT_CSP: &str = "default-src 'self'; \
script-src 'self' 'unsafe-inline' 'unsafe-eval'; \
style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; \
img-src 'self' data: blob:; \
font-src 'self' data: https://fonts.gstatic.com; \
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
        // Legacy clickjacking guard — `frame-ancestors 'none'` in the CSP
        // supersedes this on modern browsers; both ship for coverage on
        // older clients.
        SetResponseHeaderLayer::overriding(
            HeaderName::from_static("x-frame-options"),
            HeaderValue::from_static("DENY"),
        ),
        // Keeps full URLs (which may include search queries or book ids)
        // from leaking to other origins, while preserving same-origin
        // referrers used for analytics on internal navigation.
        SetResponseHeaderLayer::overriding(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ),
        // Globally suppresses MIME sniffing. The per-handler `nosniff` on
        // cover/thumb responses in `backend::covers` becomes redundant once
        // this layer is mounted, but stays as belt-and-suspenders.
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
/// flag so the two policies can't drift; sending HSTS over plain HTTP is
/// ignored by browsers but would advertise the wrong policy on a LAN-IP dev
/// origin where the operator has set `OMNIBUS_SECURE_COOKIES=0`.
pub fn hsts_layer(secure_cookies: bool) -> Option<SetResponseHeaderLayer<HeaderValue>> {
    secure_cookies.then(|| {
        SetResponseHeaderLayer::overriding(
            header::STRICT_TRANSPORT_SECURITY,
            HeaderValue::from_static(DEFAULT_HSTS),
        )
    })
}

#[cfg(test)]
mod tests;
