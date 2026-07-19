//! Admin log-viewer wire types.
//!
//! Produced by `db::logs` from the on-disk rolling JSON logs and served to
//! the admin `/logs` page. A [`LogQuery`] carries the level/module/time-range
//! filters and page cursor; a [`LogPage`] returns one newest-first page of
//! [`LogRecord`]s plus a `has_more` flag.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// Default page size when a query leaves `per_page` at 0.
pub const LOG_PER_PAGE_DEFAULT: u32 = 50;

/// Upper bound on page size — a query asking for more is clamped to this so a
/// single request can't force the reader to buffer an unbounded slice.
pub const LOG_PER_PAGE_MAX: u32 = 200;

/// One parsed structured-log record surfaced to the admin log viewer.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct LogRecord {
    /// ISO-8601 UTC timestamp exactly as the subscriber wrote it. Fixed-width
    /// and lexicographically ordered, so string comparison is chronological.
    pub timestamp: String,
    /// Level string as written: `ERROR` / `WARN` / `INFO` / `DEBUG` / `TRACE`.
    pub level: String,
    /// Event target — the emitting module path (the "module" filter matches
    /// against this).
    pub target: String,
    /// The event's `message` field, or empty when the event carried none.
    pub message: String,
    /// Compact JSON of the event's remaining fields plus request span context
    /// (method/path), for an expandable detail row. `None` when there was
    /// nothing beyond the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fields: Option<String>,
}

/// Filter + pagination parameters for a log-viewer query. Every filter is
/// optional; an absent or empty filter does not narrow the results.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogQuery {
    /// Exact level match (case-insensitive), e.g. `ERROR`. `None`/empty keeps
    /// every level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    /// Case-insensitive substring the `target` must contain, e.g.
    /// `omnibus::backend`. `None`/empty keeps every module.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub module: Option<String>,
    /// Inclusive lower bound on `timestamp`, an ISO-8601 prefix
    /// (e.g. `2026-07-19T00:00`). Compared lexicographically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from: Option<String>,
    /// Inclusive upper bound on `timestamp`, an ISO-8601 prefix. A record
    /// whose timestamp starts with `to` still matches, so a to-the-minute
    /// bound includes that whole minute.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
    /// Zero-based page index.
    #[serde(default)]
    pub page: u32,
    /// Requested page size; 0 means [`LOG_PER_PAGE_DEFAULT`], and any value is
    /// clamped to [`LOG_PER_PAGE_MAX`].
    #[serde(default)]
    pub per_page: u32,
}

/// One page of matching log records, newest first.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LogPage {
    /// Matching records for this page, newest first.
    pub records: Vec<LogRecord>,
    /// Echo of the resolved zero-based page index.
    pub page: u32,
    /// Echo of the resolved (clamped) page size.
    pub per_page: u32,
    /// Whether at least one further record matched beyond this page.
    pub has_more: bool,
}

impl LogQuery {
    /// Resolve the effective page size: 0 → default, otherwise clamped to the
    /// max. Kept here so the client and server agree on the bound.
    pub fn effective_per_page(&self) -> u32 {
        if self.per_page == 0 {
            LOG_PER_PAGE_DEFAULT
        } else {
            self.per_page.min(LOG_PER_PAGE_MAX)
        }
    }
}
