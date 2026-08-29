//! Physical Check-In wire types shared between web (`#[server]`) and mobile
//! (`reqwest`): a library-wide physical copy of a book and a per-user physical
//! wishlist entry. Both reference a book by its durable `books.uuid`.

use serde::{Deserialize, Serialize};

/// A physical copy of a book, owned library-wide (shared by all users like a
/// digital file). A book can have many; each is individually deletable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub struct PhysicalCopy {
    pub id: i64,
    pub book_uuid: String,
    /// Scanned ISBN for this copy; `None` for a manual (non-scan) add.
    pub isbn: Option<String>,
    /// User who checked the copy in; `None` if that account was later deleted.
    pub added_by_user_id: Option<i64>,
    pub checked_in_at: i64,
    /// Free-text edition/publisher note until an edition-metadata provider exists.
    pub note: Option<String>,
}

/// Where a wishlist entry was added from. Persisted as the `source` column.
///
/// The check-in flow has three front doors and they are not interchangeable
/// to a reader: a barcode read off the cover, an ISBN typed at the keypad,
/// and a title search whose ISBN came from the *provider* rather than from
/// the reader at all (#2247).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[cfg_attr(feature = "schemars", derive(schemars::JsonSchema))]
pub enum WishlistSource {
    /// Added after reading a barcode with the camera.
    Scan,
    /// Added from a book's detail page.
    Detail,
    /// Added after an ISBN was typed by hand rather than scanned.
    Manual,
    /// Added after a title search — the reader supplied no ISBN at all.
    Search,
}

impl WishlistSource {
    /// The stored `source` string (matches the migration's CHECK constraint).
    pub fn as_str(self) -> &'static str {
        match self {
            WishlistSource::Scan => "scan",
            WishlistSource::Detail => "detail",
            WishlistSource::Manual => "manual",
            WishlistSource::Search => "search",
        }
    }

    /// Parse a stored `source` string; `None` for an unrecognized value.
    pub fn from_db(s: &str) -> Option<Self> {
        match s {
            "scan" => Some(WishlistSource::Scan),
            "detail" => Some(WishlistSource::Detail),
            "manual" => Some(WishlistSource::Manual),
            "search" => Some(WishlistSource::Search),
            _ => None,
        }
    }
}

/// Body of the copy-note edit (`PATCH /api/physical/copies/{id}`). `None` — and
/// any blank string — clears the note.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct UpdateCopyNoteRequest {
    #[serde(default)]
    pub note: Option<String>,
}

impl UpdateCopyNoteRequest {
    /// Maximum length (in chars) of a physical copy's free-text note. Mirrors
    /// `omnibus_shared::highlight::UpdateHighlightNote::NOTE_MAX_LEN`.
    pub const NOTE_MAX_LEN: usize = 4096;

    /// Validate the note length. `None` clears the note and is always
    /// permitted. Returns `Err` with a human-readable message when the cap is
    /// exceeded. Handlers translate `Err(_)` into 400.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(ref note) = self.note {
            if note.chars().count() > Self::NOTE_MAX_LEN {
                return Err(format!("note exceeds {} characters", Self::NOTE_MAX_LEN));
            }
        }
        Ok(())
    }
}

/// A book on one user's physical wishlist. Unique per `(user_id, book_uuid)`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WishlistEntry {
    pub id: i64,
    pub user_id: i64,
    pub book_uuid: String,
    pub added_at: i64,
    pub source: WishlistSource,
}

#[cfg(test)]
mod tests;
