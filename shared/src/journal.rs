//! Public per-book journal wire types shared between web and mobile clients.
//!
//! A journal entry is a free-form markdown note on a book. Entries are public:
//! every authenticated user sees every user's entries for a book (a shared
//! reading log), attributed by `author_name`. Only the owner may edit or delete
//! their own entries. The markdown body is rendered to sanitized HTML
//! server-side; the raw source rides along so the owner can edit it.

use serde::{Deserialize, Serialize};

/// Maximum byte length of a journal entry's markdown source.
pub const BODY_MAX_LEN: usize = 64 * 1024;

/// Inclusive upper bound for the optional per-entry reading-progress percent.
pub const PROGRESS_MAX: u8 = 100;

/// Shared validation for a journal body + optional progress. Handlers translate
/// `Err(_)` into a 400 / 422.
fn validate_body(body_md: &str, progress: Option<u8>) -> Result<(), String> {
    if body_md.trim().is_empty() {
        return Err("journal entry cannot be empty".into());
    }
    if body_md.len() > BODY_MAX_LEN {
        return Err(format!(
            "journal entry must be {BODY_MAX_LEN} bytes or fewer"
        ));
    }
    if let Some(p) = progress {
        if p > PROGRESS_MAX {
            return Err(format!("progress must be between 0 and {PROGRESS_MAX}"));
        }
    }
    Ok(())
}

/// A persisted journal entry as rendered for display. `body_html` is the
/// server-rendered, sanitized markdown; `body_md` is the raw source (the owner
/// edits it). `created_at` / `updated_at` are unix seconds.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalEntry {
    pub id: i64,
    pub book_uuid: String,
    pub author_id: i64,
    pub author_name: String,
    pub body_md: String,
    pub body_html: String,
    pub progress: Option<u8>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Write payload: create a new journal entry on a book.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateJournalEntry {
    pub book_uuid: String,
    pub body_md: String,
    pub progress: Option<u8>,
}

impl CreateJournalEntry {
    /// Reject an empty uuid, an empty/oversized body, or out-of-range progress.
    pub fn validate(&self) -> Result<(), String> {
        if self.book_uuid.trim().is_empty() {
            return Err("book_uuid is required".into());
        }
        validate_body(&self.body_md, self.progress)
    }
}

/// Write payload: edit an existing journal entry's body and/or progress.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateJournalEntry {
    pub body_md: String,
    pub progress: Option<u8>,
}

impl UpdateJournalEntry {
    /// Reject an empty/oversized body or out-of-range progress.
    pub fn validate(&self) -> Result<(), String> {
        validate_body(&self.body_md, self.progress)
    }
}

#[cfg(test)]
mod tests;
