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

/// Publication state of a journal entry: owner-private `Draft` vs shared-feed
/// `Published` (the default, so pre-drafts clients/rows keep their behaviour).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum JournalStatus {
    Draft,
    #[default]
    Published,
}

impl JournalStatus {
    /// The lowercase form persisted in the `journal_entries.status` column.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Draft => "draft",
            Self::Published => "published",
        }
    }

    /// Parse the persisted column value; anything unrecognised reads as
    /// `Published` (fail-open matches the column's CHECK + default).
    pub fn from_db(s: &str) -> Self {
        match s {
            "draft" => Self::Draft,
            _ => Self::Published,
        }
    }
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
    #[serde(default)]
    pub status: JournalStatus,
    /// The device-minted identity this entry was created under, when the
    /// creating client supplied one. `None` for rows predating migration 0051.
    #[serde(default)]
    pub client_id: Option<String>,
    pub created_at: i64,
    pub updated_at: i64,
}

/// Write payload: create a new journal entry on a book. `status` defaults to
/// `published` so pre-drafts mobile clients keep their old behaviour.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateJournalEntry {
    pub book_uuid: String,
    pub body_md: String,
    pub progress: Option<u8>,
    #[serde(default)]
    pub status: JournalStatus,
    /// Identity minted on the device at composition time, so an offline client
    /// can edit or delete this entry before the server has assigned it an id.
    /// Creates are idempotent on it.
    #[serde(default)]
    pub client_id: Option<String>,
}

impl CreateJournalEntry {
    /// Maximum length (in chars) of a client-minted entry id. Mirror of
    /// [`crate::highlight::CreateHighlight::CLIENT_ID_MAX_LEN`].
    pub const CLIENT_ID_MAX_LEN: usize = crate::highlight::CreateHighlight::CLIENT_ID_MAX_LEN;

    /// Reject an empty uuid, an empty/oversized body, or out-of-range progress.
    pub fn validate(&self) -> Result<(), String> {
        if self.book_uuid.trim().is_empty() {
            return Err("book_uuid is required".into());
        }
        if let Some(ref client_id) = self.client_id {
            if client_id.trim().is_empty() {
                return Err("client_id must not be blank".into());
            }
            if client_id.chars().count() > Self::CLIENT_ID_MAX_LEN {
                return Err(format!(
                    "client_id exceeds {} characters",
                    Self::CLIENT_ID_MAX_LEN
                ));
            }
        }
        validate_body(&self.body_md, self.progress)
    }
}

/// Response for a journal image upload: the serving URL to embed in the
/// entry's markdown (`![caption](url)`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct JournalImageUpload {
    pub url: String,
}

/// Write payload: edit an existing journal entry's body and/or progress.
/// `status: Some(_)` transitions the entry (publish a draft); `None` — the
/// default, so pre-drafts clients are unaffected — keeps the stored status.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateJournalEntry {
    pub body_md: String,
    pub progress: Option<u8>,
    #[serde(default)]
    pub status: Option<JournalStatus>,
}

impl UpdateJournalEntry {
    /// Reject an empty/oversized body or out-of-range progress.
    pub fn validate(&self) -> Result<(), String> {
        validate_body(&self.body_md, self.progress)
    }
}

#[cfg(test)]
mod tests;
