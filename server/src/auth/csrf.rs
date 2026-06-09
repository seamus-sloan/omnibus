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

use super::{SESSION_COOKIE, SESSION_COOKIE_HOST_PREFIXED};

/// Look up either form of the session cookie in `jar`. The CSRF gate
/// triggers off the *presence* of a session cookie regardless of which
/// name the server is currently writing, so a mid-rollout
/// `OMNIBUS_SECURE_COOKIES` toggle doesn't drop CSRF protection for
/// cookies issued under the previous name.
fn has_session_cookie(jar: &CookieJar) -> bool {
    jar.get(SESSION_COOKIE_HOST_PREFIXED).is_some() || jar.get(SESSION_COOKIE).is_some()
}

/// Reject state-changing cookie-authed requests whose `Origin`/`Referer` doesn't match the allowlist or `Host`.
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
    if !has_session_cookie(&jar) {
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
mod tests;
