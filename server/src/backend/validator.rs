//! `ETag` stamping and the conditional-request preconditions that depend on
//! it, for the routes that serve a file off disk.
//!
//! `ServeFile` handles `Range` but knows nothing about entity tags, so the
//! handlers evaluate `If-None-Match` and `If-Range` here before delegating to
//! it and stamp the validator on the way back out.

use std::path::Path;

use axum::extract::Request;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use tower::ServiceExt;
use tower_http::services::ServeFile;

use super::internal;

/// `Vary` for every response that carries a validator, the 304s included.
///
/// The media routes accept either a session cookie or an `Authorization:
/// Bearer` header, so a shared cache keying only on `Cookie` could hand one
/// bearer-authenticated user's cached response — a validator match included —
/// back to a different bearer-authenticated user sharing the same (or no)
/// cookie state. Defined once so the 200, 206, and 304 paths cannot drift.
pub(super) const MEDIA_VARY: &str = "Cookie, Authorization";

/// `Cache-Control` for a validated file response.
///
/// `no-cache` means "revalidate before reuse", not "do not store": paired
/// with the `ETag` below, a client that already holds the current bytes pays
/// one conditional request and gets a bodyless 304. A `max-age` instead would
/// let a client keep serving a file that was replaced in place under the same
/// uuid-keyed URL, never even asking — the failure this module exists to fix.
pub(super) const REVALIDATE: &str = "private, no-cache";

/// Stat `path` and derive its `ETag`, or `None` when it cannot be stat'd.
///
/// Mirrors the scanner's `stat_file` exactly — whole seconds since the epoch,
/// byte length — so a validator computed here from the file on disk and one
/// the db layer computed from `book_files` are the same string whenever the
/// index is caught up with the library.
pub(super) async fn file_etag(path: &Path) -> Option<String> {
    let meta = tokio::fs::metadata(path).await.ok()?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    omnibus_shared::content_validator(mtime, meta.len() as i64)
}

/// Whether an `If-None-Match` on this request already names `etag`.
///
/// Accepts the comma-separated list form and `*`, both of which a browser can
/// legitimately send; our own clients echo back the single tag they were
/// given.
pub(super) fn if_none_match_hits(headers: &HeaderMap, etag: &str) -> bool {
    let Some(raw) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    raw.split(',')
        .map(str::trim)
        .any(|candidate| candidate == "*" || candidate == etag)
}

/// Drop a `Range` whose `If-Range` precondition no longer holds.
///
/// `ServeFile` never looks at `If-Range`, so a resumed download of a file that
/// changed underneath it would otherwise be answered with a 206 counted from
/// the *new* file — splicing a new tail onto the head the client already has
/// and leaving a corrupt book on the device, with no error raised anywhere.
/// Removing the header turns that into the full 200 the client already knows
/// how to restart from.
///
/// Only an exact match on the current entity tag keeps the range. `If-Range`
/// also admits an HTTP-date form; rather than compare clocks, anything that
/// is not the current tag counts as a miss — at worst a redundant full body,
/// never a mismatched splice.
fn enforce_if_range(mut req: Request, etag: Option<&str>) -> Request {
    let Some(requested) = req.headers().get(header::IF_RANGE) else {
        return req;
    };
    let holds = etag.is_some_and(|current| requested.as_bytes() == current.as_bytes());
    if !holds {
        req.headers_mut().remove(header::RANGE);
    }
    req
}

/// A bodyless `304`, carrying the same validator and caching headers a `200`
/// from the same handler would so a hit neither resets the client's freshness
/// bookkeeping nor opens the cross-user cache gap [`MEDIA_VARY`] closes.
pub(super) fn not_modified(etag: &str, cache_control: &str, vary: &str) -> Response {
    let mut resp = StatusCode::NOT_MODIFIED.into_response();
    let headers = resp.headers_mut();
    if let Ok(value) = HeaderValue::from_str(etag) {
        headers.insert(header::ETAG, value);
    }
    if let Ok(value) = HeaderValue::from_str(cache_control) {
        headers.insert(header::CACHE_CONTROL, value);
    }
    if let Ok(value) = HeaderValue::from_str(vary) {
        headers.insert(header::VARY, value);
    }
    resp
}

/// Serve `path` through `ServeFile` under its content validator.
///
/// Evaluates the preconditions in the order RFC 9110 gives them: an
/// `If-None-Match` that already names the current bytes short-circuits to a
/// bodyless 304, and only then does an `If-Range` decide whether a `Range`
/// survives. The 200 and 206 paths carry the `ETag`; a 404 from `ServeFile`
/// passes through untouched.
pub(super) async fn serve_file(req: Request, path: &Path) -> Response {
    let etag = file_etag(path).await;
    if let Some(etag) = etag.as_deref() {
        if if_none_match_hits(req.headers(), etag) {
            return not_modified(etag, REVALIDATE, MEDIA_VARY);
        }
    }

    let req = enforce_if_range(req, etag.as_deref());
    let res = match ServeFile::new(path).oneshot(req).await {
        Ok(r) => r,
        Err(e) => return internal("serve file", e),
    };
    let (mut parts, body) = res.into_parts();
    if matches!(parts.status, StatusCode::OK | StatusCode::PARTIAL_CONTENT) {
        if let Some(value) = etag.as_deref().and_then(|e| HeaderValue::from_str(e).ok()) {
            parts.headers.insert(header::ETAG, value);
        }
        parts
            .headers
            .insert(header::CACHE_CONTROL, HeaderValue::from_static(REVALIDATE));
        parts
            .headers
            .insert(header::VARY, HeaderValue::from_static(MEDIA_VARY));
    }
    Response::from_parts(parts, axum::body::Body::new(body))
}

#[cfg(test)]
mod tests;
