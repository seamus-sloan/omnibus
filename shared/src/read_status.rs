//! Per-user read/unread state wire types shared between web and mobile clients.
//! A book carries one [`ReadStatus`] per user — `unread` / `reading` /
//! `finished` — settable from book detail without a journal entry; a missing
//! stored row reads as `Unread` everywhere. `finished` completions feed the
//! reading-stats aggregations alongside journal-sourced ones.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// A book's read-lifecycle state for one user. Serializes as a plain lowercase
/// string (`"unread"` / `"reading"` / `"finished"`) matching the
/// `book_read_status.status` CHECK constraint.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ReadStatus {
    /// Never opened, or explicitly cleared. Also the meaning of a missing row.
    #[default]
    Unread,
    /// In progress.
    Reading,
    /// Completed — feeds the reading-stats "finished" aggregations.
    Finished,
}

impl ReadStatus {
    /// The lowercase form persisted in the `book_read_status.status` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unread => "unread",
            Self::Reading => "reading",
            Self::Finished => "finished",
        }
    }

    /// Parse a persisted/wire token; `None` if unrecognized. Named `from_str`
    /// for symmetry with `as_str` — not the fallible `FromStr` trait.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        Some(match s {
            "unread" => Self::Unread,
            "reading" => Self::Reading,
            "finished" => Self::Finished,
            _ => return None,
        })
    }

    /// Parse a column value, defaulting unrecognized strings to `Unread`
    /// (fail-open matches the column's CHECK + the "missing row = unread"
    /// convention).
    pub fn from_db(s: &str) -> Self {
        Self::from_str(s).unwrap_or(Self::Unread)
    }
}

/// Write payload: set the current user's read state for a book.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SetReadStatus {
    pub book_uuid: String,
    pub status: ReadStatus,
}

impl SetReadStatus {
    /// Reject an empty uuid. Handlers translate `Err(_)` into 400.
    pub fn validate(&self) -> Result<(), String> {
        if self.book_uuid.trim().is_empty() {
            return Err("book_uuid is required".into());
        }
        Ok(())
    }
}

/// Server-authoritative read state returned by the set/get endpoints.
/// `updated_at` is unix seconds (SQLite `strftime('%s')`); `finished_at` is the
/// moment the book most recently became `finished` (`None` otherwise).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReadStatusRecord {
    pub book_uuid: String,
    pub status: ReadStatus,
    pub updated_at: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
}
