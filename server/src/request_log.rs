//! The per-request trace layer `main.rs` mounts outermost: one INFO span per
//! request (method, redacted path, client IP, byte range, user agent), a
//! `finished processing request` event when the headers go out, and a
//! `response body finished` event when the body does — the one that turns a
//! two-hour range stream from "2 ms" into its real duration.

use std::{borrow::Cow, time::Duration};

use axum::{
    body::Body,
    http::{header, HeaderMap, HeaderName, Request, Response},
};
use tower_http::{
    classify::{ServerErrorsAsFailures, SharedClassifier},
    trace::{DefaultOnBodyChunk, DefaultOnRequest, TraceLayer},
};
use tracing::Span;

use crate::rate_limit;

#[cfg(test)]
mod tests;

/// The fully-typed layer [`layer`] returns. Plain `fn` items stand in for
/// closures so the type can be named here and the callbacks unit-tested.
pub type RequestTraceLayer = TraceLayer<
    SharedClassifier<ServerErrorsAsFailures>,
    fn(&Request<Body>) -> Span,
    DefaultOnRequest,
    fn(&Response<Body>, Duration, &Span),
    DefaultOnBodyChunk,
    fn(Option<&HeaderMap>, Duration, &Span),
>;

/// Build the request trace layer. Mount it last so it is outermost and
/// observes every response, including the 408/413 short-circuits emitted by
/// the timeout and body-limit guards inside it.
pub fn layer() -> RequestTraceLayer {
    TraceLayer::new_for_http()
        .make_span_with(make_span as fn(&Request<Body>) -> Span)
        .on_response(on_response as fn(&Response<Body>, Duration, &Span))
        .on_eos(on_eos as fn(Option<&HeaderMap>, Duration, &Span))
}

/// The request span. Logs only the path, never the query string: media
/// reads carry the session as `?token=`, and the default span records the
/// full URI. Headers never carry credentials (Authorization is deliberately
/// not among the ones recorded), so `user_agent` and `range` are safe;
/// `client_ip` honours `ConnectInfo` and the `OMNIBUS_TRUST_FORWARDED_FOR`
/// opt-in exactly as the rate limiter does.
fn make_span(req: &Request<Body>) -> Span {
    tracing::info_span!(
        "request",
        method = %req.method(),
        path = %redact_path(req.uri().path()),
        version = ?req.version(),
        client_ip = %rate_limit::client_ip(req.extensions(), req.headers()),
        range = header_str(req.headers(), header::RANGE),
        user_agent = header_str(req.headers(), header::USER_AGENT),
    )
}

/// Headers-out event. Same message and `latency` shape as tower-http's
/// `DefaultOnResponse` — external tooling and the admin log viewer key on
/// them — plus the served (or intended, for a 206) byte count. tower-http
/// enters the request span before calling this, so the event attaches to it.
fn on_response(res: &Response<Body>, latency: Duration, _span: &Span) {
    tracing::info!(
        latency = %latency_ms(latency),
        status = res.status().as_u16(),
        content_length = header_str(res.headers(), header::CONTENT_LENGTH),
        content_range = header_str(res.headers(), header::CONTENT_RANGE),
        "finished processing request"
    );
}

/// End-of-body event: how long the body actually took to stream, measured
/// from the headers going out. Fires once per response, inside the request
/// span; a body dropped before its end never reaches it.
fn on_eos(_trailers: Option<&HeaderMap>, stream_duration: Duration, _span: &Span) {
    tracing::info!(
        stream_duration_ms = duration_ms(stream_duration),
        "response body finished"
    );
}

/// A header's value as text, `""` when absent or not valid UTF-8.
pub fn header_str(headers: &HeaderMap, name: HeaderName) -> &str {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
}

/// Whole milliseconds, saturating — a `u64` so tracing records it as a number.
pub fn duration_ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

/// `"{n} ms"`, the exact shape tower-http's default `Latency` display uses.
pub fn latency_ms(d: Duration) -> String {
    format!("{} ms", duration_ms(d))
}

/// Redact the `<TOKEN>` segment of a `/kobo/<TOKEN>/…` path before it
/// reaches the trace span. Kobo devices carry a long-lived (90-day)
/// session token directly in the URL path (see
/// `backend::kobo::extractor::kobo_path_token`), so logging the raw path
/// would leak a durable credential to stderr and the on-disk JSON sink.
/// Every other path is returned unchanged, borrowed rather than
/// allocated — this runs on every request, and only the Kobo case needs
/// to build a new string.
pub fn redact_path(path: &str) -> Cow<'_, str> {
    let mut segs = path.split('/');
    match (segs.next(), segs.next(), segs.next()) {
        (Some(""), Some("kobo"), Some(_token)) => {
            let rest: String = segs.map(|s| format!("/{s}")).collect();
            Cow::Owned(format!("/kobo/[REDACTED]{rest}"))
        }
        _ => Cow::Borrowed(path),
    }
}
