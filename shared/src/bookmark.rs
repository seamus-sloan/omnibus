//! Bookmark wire types shared between web and mobile clients.
//!
//! A single bookmark model serves both surfaces. `position` is an opaque
//! location token — seconds-as-string for the audiobook player, an EPUB CFI
//! for the reader — and `title` is the optional user-entered name/note. The
//! backing `bookmarks` table (migration 0013, soft-ref'd to `book_uuid` by
//! 0027) has no dedicated note column, so `title` carries the user's text and
//! the chapter label shown in the UI is derived from `position` at render.

use serde::{Deserialize, Serialize};

/// A persisted bookmark.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bookmark {
    pub id: i64,
    pub book_uuid: String,
    pub position: String,
    pub title: Option<String>,
    pub created_at: i64,
}

/// Payload for creating a new bookmark.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateBookmark {
    pub book_uuid: String,
    pub position: String,
    pub title: Option<String>,
}

impl CreateBookmark {
    /// Maximum length (in chars) of a position token. An audiobook position is
    /// a short decimal and a CFI rarely exceeds a few hundred bytes; 4 KiB is a
    /// generous ceiling that stops an authed client persisting huge blobs.
    pub const POSITION_MAX_LEN: usize = 4096;
    /// Maximum length (in chars) of the user-entered title/note.
    pub const TITLE_MAX_LEN: usize = 512;

    /// Validate field lengths and required-ness. Call at the handler boundary
    /// before persisting so over-long inputs surface as 400 instead of falling
    /// through to the DB. Lengths are measured in Unicode scalar values.
    pub fn validate(&self) -> Result<(), String> {
        if self.book_uuid.trim().is_empty() {
            return Err("book_uuid is required".into());
        }
        if self.position.trim().is_empty() {
            return Err("position is required".into());
        }
        if self.position.chars().count() > Self::POSITION_MAX_LEN {
            return Err(format!(
                "position exceeds {} characters",
                Self::POSITION_MAX_LEN
            ));
        }
        if let Some(ref title) = self.title {
            if title.chars().count() > Self::TITLE_MAX_LEN {
                return Err(format!("title exceeds {} characters", Self::TITLE_MAX_LEN));
            }
        }
        Ok(())
    }
}

/// Payload for updating a bookmark's title/note. `None` clears the title.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateBookmark {
    pub title: Option<String>,
}

impl UpdateBookmark {
    /// Mirror of [`CreateBookmark::TITLE_MAX_LEN`] so both boundaries agree.
    pub const TITLE_MAX_LEN: usize = CreateBookmark::TITLE_MAX_LEN;

    /// Validate the title length. `None` clears the title and is always
    /// permitted. Returns `Err` with a human-readable message when over cap.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(ref title) = self.title {
            if title.chars().count() > Self::TITLE_MAX_LEN {
                return Err(format!("title exceeds {} characters", Self::TITLE_MAX_LEN));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
