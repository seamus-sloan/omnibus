//! Shared API types between the `omnibus` server and `omnibus-mobile` client.
//!
//! Keep this crate free of Dioxus and transport-layer dependencies so both
//! `#[server]` functions (web) and `reqwest` calls (mobile) can depend on it
//! without dragging in platform-specific trees.

use serde::{Deserialize, Serialize};

/// User-configurable paths for the ebook and audiobook libraries.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    pub ebook_library_path: Option<String>,
    pub audiobook_library_path: Option<String>,
}

/// Response payload for the counter endpoints (`GET /api/value`,
/// `POST /api/value/increment`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ValueResponse {
    pub value: i64,
}

/// One half of the library listing (either ebooks or audiobooks).
///
/// `counts_by_ext` is an ordered list of `(extension, count)` pairs for the
/// extensions the caller asked the scanner to track. Order matches the
/// caller-provided extension list so the UI can render a predictable summary
/// line.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibrarySection {
    pub path: Option<String>,
    pub total_files: usize,
    pub counts_by_ext: Vec<(String, usize)>,
    pub error: Option<String>,
}

/// Response payload for `GET /api/library`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryContents {
    pub ebooks: LibrarySection,
    pub audiobooks: LibrarySection,
}

/// A contributor (or creator) with the optional OPF refinements — the MARC
/// role code (`aut`, `ill`, `edt`, `bkp`, `trl`, …) and the sort-key name.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Contributor {
    pub name: String,
    pub role: Option<String>,
    pub file_as: Option<String>,
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
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EbookMetadata {
    pub id: i64,
    pub filename: String,

    // Dublin Core core — single-valued first, multi-valued second.
    pub title: Option<String>,
    pub description: Option<String>,
    pub publisher: Option<String>,
    pub published: Option<String>,
    pub modified: Option<String>,
    pub language: Option<String>,
    pub rights: Option<String>,
    pub source: Option<String>,
    pub coverage: Option<String>,
    pub dc_type: Option<String>,
    pub dc_format: Option<String>,
    pub relation: Option<String>,

    pub creators: Vec<Contributor>,
    pub contributors: Vec<Contributor>,
    pub subjects: Vec<String>,
    pub identifiers: Vec<Identifier>,

    // Series / collection (Calibre + EPUB3 belongs-to-collection).
    pub series: Option<String>,
    pub series_index: Option<String>,

    // Structural / document-level info.
    pub epub_version: Option<String>,
    pub unique_identifier: Option<String>,
    pub resource_count: usize,
    pub spine_count: usize,
    pub toc_count: usize,

    pub cover_url: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formats: Vec<String>,

    /// Row insertion timestamp from `books.timestamp` — SQLite
    /// `datetime('now')` format (`YYYY-MM-DD HH:MM:SS`, UTC, space separator).
    /// Drives the "Newest Added" sort in F1.3 — distinct from `modified`
    /// (DC last-write).
    #[serde(default)]
    pub added_at: Option<String>,

    pub error: Option<String>,
}

/// Response payload for `GET /api/ebooks` and `rpc_get_ebooks`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EbookLibrary {
    pub path: Option<String>,
    pub books: Vec<EbookMetadata>,
    pub error: Option<String>,
}

// -----------------------------------------------------------------------------
// Library view preferences (F1.3)
//
// These types live here — and not in `frontend/` — so a future server-backed
// per-user prefs endpoint can reuse them verbatim. For now persistence is
// localStorage on web and in-memory on mobile (see `frontend/src/view_prefs.rs`).
// -----------------------------------------------------------------------------

/// Which list view to render on the library page.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ViewMode {
    #[default]
    Table,
    Grid,
}

/// Sortable axes exposed in the toolbar / table headers.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SortKey {
    #[default]
    Title,
    Author,
    Series,
    LastUpdated,
    NewestAdded,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum SortDir {
    #[default]
    Asc,
    Desc,
}

/// Active filter facets. AND across facet groups; OR within a group.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ViewFilters {
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub series: Vec<String>,
}

impl ViewFilters {
    /// `true` when no facet has any selected value.
    pub fn is_empty(&self) -> bool {
        self.authors.is_empty() && self.series.is_empty()
    }
}

/// Persisted library-view preference for a single library path.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ViewPrefs {
    pub view_mode: ViewMode,
    pub sort_key: SortKey,
    pub sort_dir: SortDir,
    #[serde(default)]
    pub filters: ViewFilters,
    /// Whether the filter sidebar is open. Defaults to `false`: a brand-new
    /// visitor sees an unobstructed library and opts into filters via the
    /// toolbar's `Filters` toggle. The choice persists per library. At
    /// narrow viewports the sidebar overlays the content as a drawer when
    /// open, so leaving it closed by default avoids popping a panel over
    /// the books on first load.
    #[serde(default)]
    pub filters_open: bool,
}

// -----------------------------------------------------------------------------
// Auth (F0.3)
// -----------------------------------------------------------------------------

/// Safe projection of a `users` row. No password fields ever cross the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserSummary {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub can_upload: bool,
    pub can_edit: bool,
    pub can_download: bool,
}

/// Request body for `POST /api/auth/login`.
///
/// Deliberately does not derive `Debug`: the struct holds a plaintext
/// password, and a stray `tracing::debug!(?req)` would write it to logs.
#[derive(Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// Optional — when present, server issues a bearer session instead of a
    /// cookie session and includes the raw token in the response.
    #[serde(default)]
    pub client_kind: Option<String>,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub client_version: Option<String>,
}

/// Request body for `POST /api/auth/register`. See [`LoginRequest`] for
/// why `Debug` is deliberately not derived.
#[derive(Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub client_kind: Option<String>,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub client_version: Option<String>,
}

/// Response from `POST /api/auth/login` and `POST /api/auth/register`.
/// `token` is populated only for bearer (mobile) sessions; cookie sessions
/// return the cookie in a `Set-Cookie` header and `token` is `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub user: UserSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}
