//! Wire shape for a single ebook row.
//!
//! `EbookMetadata` is the row returned by `/api/ebooks`, `rpc_get_ebooks`,
//! and downstream detail / discovery views. `Contributor` and `Identifier`
//! are its two non-scalar sub-types.

use serde::{Deserialize, Serialize};

/// A contributor (or creator) with the optional OPF refinements — the MARC
/// role code (`aut`, `ill`, `edt`, `bkp`, `trl`, …) and the sort-key name.
///
/// `id` is the stable `authors.id` primary key from the normalized DB. Set
/// when the contributor was loaded from a m2m join; `None` for contributors
/// created by the EPUB parser before indexing.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contributor {
    pub name: String,
    pub role: Option<String>,
    pub file_as: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<i64>,
}

/// A typed identifier from the OPF, e.g. `{ scheme: "ISBN", value: "…" }`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Identifier {
    pub value: String,
    pub scheme: Option<String>,
}

/// Parsed metadata for a single ebook file.
///
/// `cover_url` is a relative URL pointing at `/api/covers/:id` when the book
/// has a cover; clients combine it with their configured server base. This
/// keeps the list response small — covers are fetched lazily as separate
/// HTTP requests instead of being embedded as base64 data URLs.
///
/// Note: OPF `<dc:contributor>` entries are merged into `creators` at parse
/// time (creators first, then contributors in source order). The normalized
/// schema stores them in the same `books_authors_link` table, so they are
/// indistinguishable on read — a separate wire field would always serialize
/// as empty.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EbookMetadata {
    pub id: i64,
    pub filename: String,

    // Dublin Core core.
    pub title: Option<String>,
    pub description: Option<String>,
    pub publisher: Option<String>,
    pub published: Option<String>,
    pub modified: Option<String>,
    pub language: Option<String>,

    pub creators: Vec<Contributor>,
    pub subjects: Vec<String>,
    pub identifiers: Vec<Identifier>,

    // Series / collection (Calibre + EPUB3 belongs-to-collection).
    pub series: Option<String>,
    pub series_index: Option<String>,
    /// Stable `series.id` primary key from the normalized DB. Set when the
    /// book was loaded from a join; `None` for books not in any series.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_id: Option<i64>,

    /// OPF unique identifier (`<dc:identifier id="…">`). Used by the
    /// frontend to construct `/books/:uuid` and `/api/covers/:uuid` URLs.
    pub unique_identifier: Option<String>,

    pub cover_url: Option<String>,

    /// Cover-derived accent color. Opaque CSS color value extracted
    /// during indexing. `None` means "no cover or extraction failed" —
    /// the frontend falls back to the theme default accent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formats: Vec<String>,

    /// Row insertion timestamp from `books.timestamp` — SQLite
    /// `datetime('now')` format (`YYYY-MM-DD HH:MM:SS`, UTC, space separator).
    /// Drives the "Newest Added" sort — distinct from `modified` (DC
    /// last-write).
    #[serde(default)]
    pub added_at: Option<String>,

    pub error: Option<String>,

    /// True when user-supplied metadata overrides are active for this book.
    /// The detail page uses this to show a "has overrides" indicator and
    /// offer a revert action.
    #[serde(default)]
    pub has_override: bool,

    /// Per-file detail for books with multiple files of the same format
    /// (e.g. five merged M4B parts, or two EPUB editions). Empty for
    /// single-file-per-format books — the `formats` vec is sufficient.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub book_files: Vec<BookFileInfo>,
}

/// One `book_files` row — a single physical file on disk. Exposed to the
/// frontend so the format switcher can offer a file picker when N > 1.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct BookFileInfo {
    pub id: i64,
    pub format: String,
    pub filename: String,
    pub ordinal: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}
