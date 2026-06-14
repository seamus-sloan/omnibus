//! Highlight annotation wire types shared between web and mobile clients.
//!
//! The five highlight colors match the CHECK constraint on
//! `highlights.color` in migration 0017.

use serde::{Deserialize, Serialize};

/// Valid highlight palette colors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HighlightColor {
    Amber,
    Green,
    Blue,
    Rose,
    Violet,
}

impl HighlightColor {
    /// Lowercase storage/wire token for this color (matches migration 0017).
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Amber => "amber",
            Self::Green => "green",
            Self::Blue => "blue",
            Self::Rose => "rose",
            Self::Violet => "violet",
        }
    }

    /// Parse a lowercase token back into a color, `None` if unrecognized.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "amber" => Some(Self::Amber),
            "green" => Some(Self::Green),
            "blue" => Some(Self::Blue),
            "rose" => Some(Self::Rose),
            "violet" => Some(Self::Violet),
            _ => None,
        }
    }
}

impl std::fmt::Display for HighlightColor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A persisted highlight annotation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Highlight {
    pub id: i64,
    pub book_uuid: String,
    pub epub_cfi_range: String,
    pub color: HighlightColor,
    pub note: Option<String>,
    pub created_at: i64,
}

/// Payload for creating a new highlight.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateHighlight {
    pub book_uuid: String,
    pub epub_cfi_range: String,
    pub color: HighlightColor,
}

impl CreateHighlight {
    /// Maximum length (in chars) of an EPUB CFI range string. A real CFI
    /// rarely exceeds a few hundred bytes; 4 KiB is a generous ceiling that
    /// stops an authed client from persisting megabyte-sized blobs per row.
    pub const EPUB_CFI_RANGE_MAX_LEN: usize = 4096;

    /// Validate field lengths and required-ness. Call at the handler
    /// boundary before persisting so over-long inputs surface as 400
    /// instead of falling through to the DB.
    ///
    /// Length caps are measured in Unicode scalar values (`chars`),
    /// matching `MetadataOverrides::validate`.
    pub fn validate(&self) -> Result<(), String> {
        if self.book_uuid.trim().is_empty() {
            return Err("book_uuid is required".into());
        }
        if self.epub_cfi_range.trim().is_empty() {
            return Err("epub_cfi_range is required".into());
        }
        if self.epub_cfi_range.chars().count() > Self::EPUB_CFI_RANGE_MAX_LEN {
            return Err(format!(
                "epub_cfi_range exceeds {} characters",
                Self::EPUB_CFI_RANGE_MAX_LEN
            ));
        }
        Ok(())
    }
}

/// Payload for updating a highlight's note text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateHighlightNote {
    pub note: Option<String>,
}

impl UpdateHighlightNote {
    /// Maximum length (in chars) of a highlight note. Matches the
    /// `MetadataOverrides.description`-style per-row text budget so the
    /// API boundary rejects megabyte payloads up front.
    pub const NOTE_MAX_LEN: usize = 4096;

    /// Validate the note length. `None` clears the note and is always
    /// permitted. Returns `Err` with a human-readable message when the
    /// cap is exceeded.
    pub fn validate(&self) -> Result<(), String> {
        if let Some(ref note) = self.note {
            if note.chars().count() > Self::NOTE_MAX_LEN {
                return Err(format!("note exceeds {} characters", Self::NOTE_MAX_LEN));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
