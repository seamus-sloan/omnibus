//! Conditional-request handling for the endpoints that serve bytes off disk.
//!
//! This module is the **single** place those endpoints evaluate
//! preconditions and honour `Range`. It exists for one guarantee: a resumed
//! download must never splice the tail of a new file onto the head of an old
//! one. Two things are load-bearing for that, and both are easy to lose:
//!
//! - The validator and the bytes come from **one open file handle**, opened
//!   once. Deriving a validator from a path and then letting something else
//!   re-open that path leaves a window in which an atomic replace slips
//!   between them — an `If-Range` that matched the old file, served from the
//!   new one. On Unix an open handle keeps pointing at the inode it was
//!   opened on, so a `rename` over the path cannot move underneath us.
//! - Every precondition is settled **here**, in RFC 9110 §13.2.2 order.
//!   Delegating any of them to a service with its own differently-derived
//!   validator produces answers that disagree with the one we published.

use std::path::Path;
use std::time::SystemTime;

use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderName, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use omnibus_shared::http_range::{self, RangeOutcome};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

/// `Vary` value shared by every response the media handlers return (200s
/// *and* 304s). Those endpoints accept either a session cookie or an
/// `Authorization: Bearer` header, so a shared/intermediate cache keying
/// only on `Cookie` could hand one bearer-authenticated user's cached
/// response — including a 304 validator match — back to a different
/// bearer-authenticated user who shares the same (or no) cookie state.
/// Defined once so the 200 and 304 paths can never drift apart.
pub(super) const MEDIA_VARY: &str = "Cookie, Authorization";

/// `Cache-Control` shared by every response these handlers return.
/// `no-cache` rather than a `max-age`: a book file can be replaced in place
/// under the same uuid-keyed URL, and a `max-age`-only cache would keep
/// serving the old bytes until it expired — the client never even asks. The
/// whole point of the `ETag` alongside it is that it does ask, and
/// revalidation costs one 304.
pub(super) const MEDIA_CACHE_CONTROL: &str = "private, no-cache";

/// Everything needed to answer conditionally, taken from one open handle.
pub(super) struct OpenFile {
    file: tokio::fs::File,
    pub(super) len: u64,
    pub(super) validator: Validator,
}

/// The identity of the bytes behind an [`OpenFile`].
pub(super) struct Validator {
    pub(super) etag: String,
    pub(super) last_modified: SystemTime,
}

/// Open `path` and derive its validator from the handle, not the path.
///
/// The inode is part of the tag deliberately. A stat-derived validator's
/// real weakness is a replacement that *preserves* the timestamp — `rsync
/// -t`, `cp -p`, a restore from backup — and almost every such tool writes a
/// temp file and renames it over the target, which moves the inode even when
/// the mtime comes along unchanged. (Apache's historic
/// `FileETag INode MTime Size`; it dropped `INode` only because it breaks
/// multi-server clusters, which a self-hosted single instance is not.)
///
/// Nanosecond precision here is intentionally finer than the second-granular
/// [`omnibus_shared::file_etag`] the db projection puts on the wire: a miss
/// on *this* validator splices a file, where a miss on that one merely
/// delays a notification.
///
/// **Known residual:** an in-place overwrite that preserves both length and
/// timestamp still slips through. No stat-derived validator catches that,
/// which is why the download client verifies the assembled file rather than
/// trusting this alone.
pub(super) async fn open(path: &Path) -> std::io::Result<OpenFile> {
    let file = tokio::fs::File::open(path).await?;
    let meta = file.metadata().await?;
    let last_modified = meta.modified()?;
    let duration = last_modified
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_err(|_| std::io::Error::other("pre-epoch modification time"))?;
    let etag = format!(
        "\"{}{:x}.{:08x}-{:x}\"",
        inode_prefix(&meta),
        duration.as_secs(),
        duration.subsec_nanos(),
        meta.len()
    );
    Ok(OpenFile {
        file,
        len: meta.len(),
        validator: Validator {
            etag,
            last_modified,
        },
    })
}

/// `"{inode:x}-"` where the platform exposes one, else empty.
fn inode_prefix(meta: &std::fs::Metadata) -> String {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        format!("{:x}-", meta.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        String::new()
    }
}

/// Outcome of evaluating a request's preconditions.
pub(super) enum Precondition {
    /// `If-None-Match` (or `If-Modified-Since`) says the client's copy is
    /// current. The validator rides along so the caller can build its 304
    /// via [`not_modified`] without re-proving one existed.
    NotModified(String),
    /// `If-Match` / `If-Unmodified-Since` failed: 412, no body.
    Failed,
    /// Serve the representation. `range` is already resolved against the
    /// file's length, and is [`RangeOutcome::Full`] whenever the request's
    /// `If-Range` went stale.
    Proceed { range: RangeOutcome },
}

/// Evaluate every precondition against `validator`, in RFC 9110 §13.2.2
/// precedence order, and resolve `Range` against `len`.
///
/// The precedence matters and is the reason this cannot be split across two
/// layers: a stale `If-None-Match` *must* produce the full body even when an
/// `If-Modified-Since` on the same request would have matched, because
/// step 4 only runs when step 3's header is absent. Evaluating the ETag
/// conditions here and leaving the date conditions to a downstream file
/// service gets that exactly backwards, and answers 304 to a client that
/// just told us its copy is out of date.
pub(super) fn evaluate(headers: &HeaderMap, validator: &Validator, len: u64) -> Precondition {
    // 1. If-Match — strong comparison.
    if let Some(value) = header_str(headers, header::IF_MATCH) {
        if !if_match_passes(value, &validator.etag) {
            return Precondition::Failed;
        }
    } else if let Some(value) = header_str(headers, header::IF_UNMODIFIED_SINCE) {
        // 2. If-Unmodified-Since — only when If-Match is absent.
        match httpdate::parse_http_date(value) {
            Ok(since) if truncate_secs(validator.last_modified) > truncate_secs(since) => {
                return Precondition::Failed
            }
            // An unparseable date must be ignored, not treated as a failure.
            _ => {}
        }
    }

    // 3. If-None-Match — weak comparison.
    if let Some(value) = header_str(headers, header::IF_NONE_MATCH) {
        if if_none_match_hits(value, &validator.etag) {
            return Precondition::NotModified(validator.etag.clone());
        }
        // Present but not matching: the client's copy is stale, so step 4 is
        // skipped entirely and the full body is owed.
        return Precondition::Proceed {
            range: resolve_range(headers, validator, len),
        };
    }
    // 4. If-Modified-Since — only when If-None-Match is absent.
    if let Some(value) = header_str(headers, header::IF_MODIFIED_SINCE) {
        if let Ok(since) = httpdate::parse_http_date(value) {
            if truncate_secs(validator.last_modified) <= truncate_secs(since) {
                return Precondition::NotModified(validator.etag.clone());
            }
        }
    }

    Precondition::Proceed {
        range: resolve_range(headers, validator, len),
    }
}

/// The `Range` to serve, or [`RangeOutcome::Full`] when an `If-Range` no
/// longer matches. A resource whose `If-Range` went stale is the splice this
/// module exists to prevent, and "serve the whole current body" is the only
/// safe answer.
fn resolve_range(headers: &HeaderMap, validator: &Validator, len: u64) -> RangeOutcome {
    if let Some(value) = header_str(headers, header::IF_RANGE) {
        if !if_range_matches(value, validator) {
            return RangeOutcome::Full;
        }
    }
    http_range::parse(header_str(headers, header::RANGE), len)
}

/// Build a `304 Not Modified` carrying the same `Cache-Control` / `Vary` a
/// 200 from the same endpoint would, so a hit neither resets the client's
/// notion of freshness nor opens the cross-user cache gap [`MEDIA_VARY`]
/// documents.
pub(super) fn not_modified(etag: &str, cache_control: &str, vary: &str) -> Response {
    let mut resp = StatusCode::NOT_MODIFIED.into_response();
    let headers = resp.headers_mut();
    for (name, value) in [
        (header::CACHE_CONTROL, cache_control),
        (header::ETAG, etag),
        (header::VARY, vary),
    ] {
        if let Ok(value) = HeaderValue::from_str(value) {
            headers.insert(name, value);
        }
    }
    resp
}

/// How the body should be presented to the client.
pub(super) struct FileResponse<'a> {
    pub(super) content_type: &'a str,
    pub(super) cache_control: &'a str,
    /// `Content-Disposition` value, for the download (as opposed to inline)
    /// endpoints.
    pub(super) disposition: Option<String>,
}

/// Stream `open`'s bytes as 200, 206, or 416, from the handle its validator
/// was taken from.
///
/// A 416 is answered here rather than passed along from something else, and
/// carries the `Content-Range: bytes */{len}` a resuming client needs to
/// restart correctly — reporting it as 404 would tell that client the book
/// had disappeared.
pub(super) async fn serve(open: OpenFile, range: RangeOutcome, spec: FileResponse<'_>) -> Response {
    let OpenFile {
        mut file,
        len,
        validator,
    } = open;

    let (status, start, span) = match range {
        RangeOutcome::Full => (StatusCode::OK, 0, len),
        RangeOutcome::Partial { start, end } => {
            (StatusCode::PARTIAL_CONTENT, start, end - start + 1)
        }
        RangeOutcome::NotSatisfiable => {
            let mut resp = StatusCode::RANGE_NOT_SATISFIABLE.into_response();
            insert_headers(
                resp.headers_mut(),
                &[
                    (header::CONTENT_RANGE, format!("bytes */{len}")),
                    (header::ACCEPT_RANGES, "bytes".into()),
                    (header::CACHE_CONTROL, spec.cache_control.into()),
                    (header::VARY, MEDIA_VARY.into()),
                    (header::ETAG, validator.etag),
                ],
            );
            return resp;
        }
    };

    if start > 0 && file.seek(std::io::SeekFrom::Start(start)).await.is_err() {
        return super::internal("seek media file", "seek failed");
    }

    let body = Body::from_stream(ReaderStream::new(file.take(span)));
    let mut resp = Response::builder()
        .status(status)
        .body(body)
        .unwrap_or_else(|_| super::internal("build media response", "response build failed"));

    let mut fields = vec![
        (header::CONTENT_TYPE, spec.content_type.to_string()),
        (header::CONTENT_LENGTH, span.to_string()),
        (header::ACCEPT_RANGES, "bytes".into()),
        (header::CACHE_CONTROL, spec.cache_control.into()),
        (header::VARY, MEDIA_VARY.into()),
        (header::ETAG, validator.etag),
        (
            header::LAST_MODIFIED,
            httpdate::fmt_http_date(validator.last_modified),
        ),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff".into()),
    ];
    if status == StatusCode::PARTIAL_CONTENT {
        fields.push((
            header::CONTENT_RANGE,
            format!("bytes {start}-{}/{len}", start + span - 1),
        ));
    }
    if let Some(disposition) = spec.disposition {
        fields.push((header::CONTENT_DISPOSITION, disposition));
    }
    insert_headers(resp.headers_mut(), &fields);
    resp
}

fn insert_headers(headers: &mut HeaderMap, fields: &[(HeaderName, String)]) {
    for (name, value) in fields {
        if let Ok(value) = HeaderValue::from_str(value) {
            headers.insert(name.clone(), value);
        }
    }
}

/// HTTP dates carry whole seconds, so a finer-grained mtime has to be
/// truncated before it is compared against one — otherwise a file written
/// mid-second always reads as "modified after" a date describing it.
fn truncate_secs(time: SystemTime) -> SystemTime {
    match time.duration_since(SystemTime::UNIX_EPOCH) {
        Ok(d) => SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(d.as_secs()),
        Err(_) => time,
    }
}

fn header_str(headers: &HeaderMap, name: HeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

/// `If-Match` (RFC 9110 §13.1.1): `*`, or any list entry matching under
/// *strong* comparison.
fn if_match_passes(value: &str, etag: &str) -> bool {
    let value = value.trim();
    if value == "*" {
        return true;
    }
    value
        .split(',')
        .any(|candidate| strong_eq(candidate.trim(), etag))
}

/// `If-None-Match` (RFC 9110 §13.1.2): `*`, or any list entry that matches
/// under weak comparison. `pub(super)` so the cover/thumb handlers — which
/// hash their bytes rather than stat a path, and so build their own
/// validator — share this one parser instead of a second `==` on the raw
/// header value.
pub(super) fn if_none_match_hits(value: &str, etag: &str) -> bool {
    let value = value.trim();
    if value == "*" {
        return true;
    }
    value
        .split(',')
        .any(|candidate| weak_eq(candidate.trim(), etag))
}

/// `If-Range` (RFC 9110 §13.1.5): a single entity-tag under *strong*
/// comparison, or an exact `Last-Modified` date. Anything else — a weak tag,
/// an unparseable date — falls through to the safe answer: full body, no
/// splice.
fn if_range_matches(value: &str, validator: &Validator) -> bool {
    let value = value.trim();
    if value.starts_with('"') || value.starts_with("W/") {
        return strong_eq(value, &validator.etag);
    }
    match httpdate::parse_http_date(value) {
        Ok(date) => truncate_secs(validator.last_modified) == truncate_secs(date),
        Err(_) => false,
    }
}

/// Strong comparison per RFC 9110 §8.8.3.2: neither side may be weak.
fn strong_eq(candidate: &str, etag: &str) -> bool {
    !candidate.starts_with("W/") && !etag.starts_with("W/") && candidate == etag
}

/// Weak comparison per RFC 9110 §8.8.3.2: ignore a `W/` prefix on either
/// side, then compare the opaque tags.
fn weak_eq(candidate: &str, etag: &str) -> bool {
    strip_weak(candidate) == strip_weak(etag)
}

fn strip_weak(tag: &str) -> &str {
    tag.strip_prefix("W/").unwrap_or(tag)
}

#[cfg(test)]
mod tests;
