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
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Amber => "amber",
            Self::Green => "green",
            Self::Blue => "blue",
            Self::Rose => "rose",
            Self::Violet => "violet",
        }
    }

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

/// Payload for updating a highlight's note text.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UpdateHighlightNote {
    pub note: Option<String>,
}
