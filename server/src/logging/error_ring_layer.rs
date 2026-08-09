//! Tracing `Layer` feeding the in-memory error ring buffer.
//!
//! Installed alongside the stderr/JSON-file layers in
//! [`crate::logging::init_tracing`]. Captures every `ERROR`-level event in
//! the process with no explicit call site required at each error (AC3),
//! extracting the message plus any book/file identifier field so the admin
//! "Last errors" section can link the offending item.
//!
//! **Redaction convention.** The captured `message` is served verbatim over
//! `GET /api/rpc/errors` to any admin session — network-reachable, unlike
//! the durable JSON log sink it complements. `FieldVisitor` below applies no
//! redaction, so every `tracing::error!` call site is responsible for never
//! string-interpolating a secret-bearing value (an API key, a password, an
//! SMTP credential) into the message literal — pass it as a named field
//! instead, e.g. `error = %e`, which this layer deliberately does not
//! capture. See #1777.

use omnibus_db::error_ring::{self, CapturedError};
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_subscriber::layer::Context;
use tracing_subscriber::Layer;

#[cfg(test)]
mod tests;

/// Field names whose value is captured as the entry's `book_uuid`.
const BOOK_UUID_FIELDS: &[&str] = &["book_uuid", "uuid"];
/// Field names whose value is captured as the entry's `file`.
const FILE_FIELDS: &[&str] = &["file", "path", "file_path"];

/// Tracing layer that records every `ERROR`-level event into
/// [`omnibus_db::error_ring`]. Every other level is a no-op.
pub struct ErrorRingLayer;

impl<S: Subscriber> Layer<S> for ErrorRingLayer {
    fn on_event(&self, event: &Event<'_>, _ctx: Context<'_, S>) {
        if *event.metadata().level() != Level::ERROR {
            return;
        }
        let mut visitor = FieldVisitor::default();
        event.record(&mut visitor);
        error_ring::record(CapturedError {
            timestamp: now_unix_secs(),
            target: event.metadata().target().to_string(),
            message: visitor.message,
            book_uuid: visitor.book_uuid,
            file: visitor.file,
        });
    }
}

/// Pulls the `message` field plus any known book/file identifier field out
/// of an event's fields. Unrecognized fields are dropped — the ring buffer
/// is a lightweight recent-errors view, not a full structured-log record
/// (that's `db::logs`, reading the durable JSON sink).
#[derive(Default)]
struct FieldVisitor {
    message: String,
    book_uuid: Option<String>,
    file: Option<String>,
}

impl FieldVisitor {
    /// Route one recorded `(name, value)` pair to the matching entry field.
    fn capture(&mut self, name: &str, value: &str) {
        // `?field` (Debug) on a `String`/`PathBuf` wraps the value in
        // quotes; strip them so the captured identifier matches what a
        // `%field` (Display) call site would have produced.
        let value = value.trim_matches('"');
        if name == "message" {
            self.message = value.to_string();
        } else if BOOK_UUID_FIELDS.contains(&name) {
            self.book_uuid = Some(value.to_string());
        } else if FILE_FIELDS.contains(&name) {
            self.file = Some(value.to_string());
        }
    }
}

impl Visit for FieldVisitor {
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        self.capture(field.name(), &format!("{value:?}"));
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.capture(field.name(), value);
    }
}

/// Current unix time in seconds. `unwrap_or(0)` rather than panicking on the
/// (practically unreachable) pre-1970 clock case — mirrors
/// `backend::health`'s `now_unix_secs`.
fn now_unix_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
