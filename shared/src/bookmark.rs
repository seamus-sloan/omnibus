//! Bookmark wire types shared between web and mobile clients. One model
//! serves both surfaces: `position` is an opaque location token (seconds for
//! the audiobook player, an EPUB CFI for the reader) and `title` is the
//! optional user name/note (the `bookmarks` table has no separate note column).

use serde::{Deserialize, Serialize};

use crate::highlight::CreateHighlight;

/// A persisted bookmark.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Bookmark {
    pub id: i64,
    pub book_uuid: String,
    pub position: String,
    pub title: Option<String>,
    /// The device-minted identity this bookmark was created under, when the
    /// creating client supplied one. `None` for rows predating migration 0049.
    #[serde(default)]
    pub client_id: Option<String>,
    pub created_at: i64,
}

/// Payload for creating a new bookmark.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateBookmark {
    pub book_uuid: String,
    pub position: String,
    pub title: Option<String>,
    /// Identity minted on the device at the moment of the gesture, so an
    /// offline client can address this bookmark before the server has assigned
    /// it a numeric id. Creates are idempotent on it.
    #[serde(default)]
    pub client_id: Option<String>,
}

impl CreateBookmark {
    /// Maximum length (in chars) of a position token. An audiobook position is
    /// a short decimal and a CFI rarely exceeds a few hundred bytes; 4 KiB is a
    /// generous ceiling that stops an authed client persisting huge blobs.
    pub const POSITION_MAX_LEN: usize = 4096;
    /// Maximum length (in chars) of the user-entered title/note.
    pub const TITLE_MAX_LEN: usize = 512;

    /// Mirror of [`CreateHighlight::CLIENT_ID_MAX_LEN`] — both hold the same
    /// kind of device-minted handle, so the two ceilings can't drift apart.
    pub const CLIENT_ID_MAX_LEN: usize = CreateHighlight::CLIENT_ID_MAX_LEN;

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
