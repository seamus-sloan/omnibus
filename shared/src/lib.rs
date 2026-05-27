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
    /// Stable `series.id` primary key from the normalized DB. Set when the
    /// book was loaded from a join; `None` for books not in any series.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_id: Option<i64>,

    // Structural / document-level info.
    pub epub_version: Option<String>,
    pub unique_identifier: Option<String>,
    pub resource_count: usize,
    pub spine_count: usize,
    pub toc_count: usize,

    pub cover_url: Option<String>,

    /// Cover-derived accent color (F1.7 Atrium). Opaque CSS color value
    /// extracted during indexing. `None` means "no cover or extraction
    /// failed" — the frontend falls back to the theme default accent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,

    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub formats: Vec<String>,

    /// Row insertion timestamp from `books.timestamp` — SQLite
    /// `datetime('now')` format (`YYYY-MM-DD HH:MM:SS`, UTC, space separator).
    /// Drives the "Newest Added" sort in F1.3 — distinct from `modified`
    /// (DC last-write).
    #[serde(default)]
    pub added_at: Option<String>,

    pub error: Option<String>,

    /// True when user-supplied metadata overrides (F5.1) are active for this
    /// book. The detail page uses this to show a "has overrides" indicator and
    /// offer a revert action.
    #[serde(default)]
    pub has_override: bool,
}

/// User-supplied metadata overrides (F5.1). JSON-serialized into the
/// `metadata_overrides.overrides` column. Each field, when `Some`, replaces
/// the corresponding scanned value at read time.
///
/// M2M fields (`creators`, `subjects`) replace entirely when present — a tag
/// list override replaces, not appends (per roadmap: "A tag list override
/// should replace, not append").
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct MetadataOverrides {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub publisher: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub series_index: Option<String>,
    /// Replaces the entire creators list when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub creators: Option<Vec<Contributor>>,
    /// Replaces the entire subjects (tags) list when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subjects: Option<Vec<String>>,
}

impl MetadataOverrides {
    /// Maximum length for scalar string fields (title, publisher, etc.).
    const MAX_SCALAR_LEN: usize = 4096;
    /// Maximum length for the description field.
    const MAX_DESCRIPTION_LEN: usize = 256 * 1024;
    /// Maximum number of tags (subjects).
    const MAX_SUBJECTS: usize = 64;
    /// Maximum length of a single tag.
    const MAX_SUBJECT_LEN: usize = 128;
    /// Maximum number of creators.
    const MAX_CREATORS: usize = 32;
    /// Maximum length of a creator name.
    const MAX_CREATOR_LEN: usize = 256;

    /// Validate field lengths. Returns `Err` with a human-readable message if
    /// any field exceeds the cap. Call at the handler level before persisting.
    pub fn validate(&self) -> Result<(), String> {
        let check_scalar = |name: &str, val: &Option<String>, max: usize| -> Result<(), String> {
            if let Some(v) = val {
                if v.len() > max {
                    return Err(format!("{name} exceeds {max} characters"));
                }
            }
            Ok(())
        };
        check_scalar("title", &self.title, Self::MAX_SCALAR_LEN)?;
        check_scalar("publisher", &self.publisher, Self::MAX_SCALAR_LEN)?;
        check_scalar("published", &self.published, Self::MAX_SCALAR_LEN)?;
        check_scalar("language", &self.language, Self::MAX_SCALAR_LEN)?;
        check_scalar("series", &self.series, Self::MAX_SCALAR_LEN)?;
        check_scalar("series_index", &self.series_index, Self::MAX_SCALAR_LEN)?;
        check_scalar("description", &self.description, Self::MAX_DESCRIPTION_LEN)?;
        if let Some(ref creators) = self.creators {
            if creators.len() > Self::MAX_CREATORS {
                return Err(format!("too many creators (max {})", Self::MAX_CREATORS));
            }
            for c in creators {
                if c.name.len() > Self::MAX_CREATOR_LEN {
                    return Err(format!(
                        "creator name exceeds {} characters",
                        Self::MAX_CREATOR_LEN
                    ));
                }
            }
        }
        if let Some(ref subjects) = self.subjects {
            if subjects.len() > Self::MAX_SUBJECTS {
                return Err(format!("too many tags (max {})", Self::MAX_SUBJECTS));
            }
            for s in subjects {
                if s.len() > Self::MAX_SUBJECT_LEN {
                    return Err(format!("tag exceeds {} characters", Self::MAX_SUBJECT_LEN));
                }
            }
        }
        Ok(())
    }

    /// Merge `incoming` on top of `self`. Fields that are `Some` in `incoming`
    /// win; `None` fields in `incoming` preserve `self`'s value. This lets a
    /// second edit that only touches two fields preserve earlier overrides on
    /// untouched fields.
    pub fn merge(&self, incoming: &MetadataOverrides) -> MetadataOverrides {
        MetadataOverrides {
            title: incoming.title.clone().or(self.title.clone()),
            description: incoming.description.clone().or(self.description.clone()),
            publisher: incoming.publisher.clone().or(self.publisher.clone()),
            published: incoming.published.clone().or(self.published.clone()),
            language: incoming.language.clone().or(self.language.clone()),
            series: incoming.series.clone().or(self.series.clone()),
            series_index: incoming.series_index.clone().or(self.series_index.clone()),
            creators: incoming.creators.clone().or(self.creators.clone()),
            subjects: incoming.subjects.clone().or(self.subjects.clone()),
        }
    }
}

/// Response payload for `GET /api/ebooks` and `rpc_get_ebooks`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EbookLibrary {
    pub path: Option<String>,
    pub books: Vec<EbookMetadata>,
    pub error: Option<String>,
}

// -----------------------------------------------------------------------------
// Discovery pages (F1.8)
// -----------------------------------------------------------------------------

/// Author detail payload for `GET /api/authors/:id` and `rpc_get_author`.
/// Contains the author row plus every book by that author.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthorDetail {
    pub id: i64,
    pub name: String,
    pub sort: Option<String>,
    pub book_count: usize,
    pub books: Vec<EbookMetadata>,
    /// F1.11: `true` when a usable profile photo is cached for this author
    /// (a `manual` or `openlibrary` row in `author_photos`). The frontend
    /// hero swaps the letter avatar for `<img src="/api/authors/:id/photo">`
    /// when set. `'letter'` negative-cache rows do not set this flag.
    #[serde(default)]
    pub has_photo: bool,
}

/// Result of an admin-triggered "Scan for picture" run (F1.11). The endpoint
/// clears any cached row and runs the Open Library cascade inline; a
/// `false` here means Open Library had nothing to offer for this author
/// and a sticky `letter` marker has been written to skip future
/// autoresolution.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthorPhotoScanResult {
    pub resolved: bool,
}

/// Series detail payload for `GET /api/series/:id` and `rpc_get_series`.
/// Books are ordered by `series_index`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeriesDetail {
    pub id: i64,
    pub name: String,
    pub sort: Option<String>,
    pub book_count: usize,
    pub books: Vec<EbookMetadata>,
}

/// Single tag with its book count, for the tag cloud.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct TagWeight {
    pub name: String,
    pub count: usize,
}

/// Lightweight author row for the `/authors` index (F1.12). Carries only
/// what the card needs — no joined book list. The detail page
/// (`AuthorDetail`) is fetched on click.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthorSummary {
    pub id: i64,
    pub name: String,
    pub sort: Option<String>,
    pub book_count: usize,
    /// Cover-derived accent color borrowed from the author's first book
    /// with a non-null `accent_color`. `None` when no owned book has one
    /// — the UI falls back to the theme accent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
    /// F1.11: `true` when a usable profile photo is cached for this author
    /// (a `manual` or `openlibrary` row in `author_photos`). Same semantics
    /// as `AuthorDetail::has_photo` — `'letter'` negative-cache rows do
    /// not set this flag. Lets the `/authors` index render a real `<img>`
    /// in one round trip instead of fetching the detail payload per card.
    #[serde(default)]
    pub has_photo: bool,
}

/// Lightweight series row for the `/series` index (F1.12). Carries the
/// primary author of the series (first book's first creator) so the card
/// can render the by-line without a second fetch.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeriesSummary {
    pub id: i64,
    pub name: String,
    pub sort: Option<String>,
    pub book_count: usize,
    /// Primary author display string (first book's first creator). `None`
    /// when the series has no books with a linked author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_author: Option<String>,
    /// Accent color borrowed from the first book in the series with one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub accent: Option<String>,
}

// -----------------------------------------------------------------------------
// Search palette (F1.5)
//
// Grouped results for the command-palette overlay. Slim types — the palette
// needs display data only, not the full `EbookMetadata` with its N+1 queries.
// Each category is capped at 5 results server-side.
// -----------------------------------------------------------------------------

/// Search-palette response — grouped results with server-side timing.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PaletteResults {
    pub query: String,
    pub books: Vec<PaletteBookHit>,
    pub authors: Vec<PaletteAuthorHit>,
    pub series: Vec<PaletteSeriesHit>,
    pub tags: Vec<PaletteTagHit>,
    pub duration_ms: u64,
}

impl PaletteResults {
    pub fn total_count(&self) -> usize {
        self.books.len() + self.authors.len() + self.series.len() + self.tags.len()
    }
}

/// Slim book hit for the search palette. No description, no identifiers,
/// no subjects — just what the result row needs to render.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PaletteBookHit {
    pub id: i64,
    /// Stable UUID — what the frontend should use to construct
    /// `/books/:uuid` and `/api/covers/:uuid` URLs.
    #[serde(default)]
    pub uuid: String,
    pub title: String,
    /// Pre-joined author names (e.g. "Grace Hopper, Margaret Hamilton").
    pub author_display: String,
    /// Four-digit year extracted from `pubdate`, if present.
    pub year: Option<String>,
    pub formats: Vec<String>,
    pub cover_url: Option<String>,
    pub accent: Option<String>,
}

/// Author hit for the search palette.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PaletteAuthorHit {
    pub id: i64,
    pub name: String,
    /// Number of books by this author in the active library.
    pub book_count: u32,
}

/// Series hit for the search palette.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PaletteSeriesHit {
    pub id: i64,
    pub name: String,
    pub book_count: u32,
    /// Primary author of the first book in the series, if any.
    pub author_display: Option<String>,
}

/// Tag hit for the search palette.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PaletteTagHit {
    pub id: i64,
    pub name: String,
    /// Number of books with this tag in the active library.
    pub book_count: u32,
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
///
/// Format values are stored lowercase (`"epub"`, `"m4b"`) since the underlying
/// `EbookMetadata.formats` strings vary in case across sources.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ViewFilters {
    #[serde(default)]
    pub authors: Vec<String>,
    #[serde(default)]
    pub series: Vec<String>,
    #[serde(default)]
    pub formats: Vec<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

impl ViewFilters {
    /// `true` when no facet has any selected value.
    pub fn is_empty(&self) -> bool {
        self.authors.is_empty()
            && self.series.is_empty()
            && self.formats.is_empty()
            && self.tags.is_empty()
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

/// Discriminant for [`TaskProgress`] entries (F0.5 worker). Mirrors the
/// payload-less shape of the server-side `Task` enum so the wire protocol
/// stays decoupled from the runtime `Task` type's lifetimes and handler
/// closures. `#[non_exhaustive]` so future worker actions (e.g. F5.9
/// `DetectCleanup`) can be added without breaking client matches.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[non_exhaustive]
pub enum TaskKind {
    Scan,
    GenerateThumbs,
    ResolveAuthorPhoto,
}

/// Lifecycle state of a single worker task as exposed to the UI.
///
/// `Running.total = None` means the task either has no granular progress
/// surface yet or is in a pre-count phase (e.g. scanner tree-walk). When
/// `total` is `Some`, `processed / total` is a stable ratio the UI can
/// render as a progress bar. Terminal variants (`Done`, `Failed`) stick
/// around in [`WorkerStatus::recent_complete`] for ~10s after completion
/// so a transient "Library updated" / error banner can render before the
/// indicator collapses.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum ProgressState {
    Running {
        processed: u32,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        total: Option<u32>,
    },
    Done {
        processed: u32,
    },
    Failed {
        message: String,
    },
}

impl ProgressState {
    /// Terminal states (`Done`, `Failed`) live in the "recently completed"
    /// bucket; non-terminal states live in "active." Used by both the worker
    /// snapshot partitioner and the client renderer.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            ProgressState::Done { .. } | ProgressState::Failed { .. }
        )
    }
}

/// One row of the worker progress feed. `task_id` is the process-local
/// worker id (not stable across server restarts); the UI uses it only as a
/// stable key for list rendering and dismiss-tracking.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskProgress {
    pub task_id: u64,
    pub kind: TaskKind,
    pub state: ProgressState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resource_key: Option<String>,
    /// Milliseconds since the UNIX epoch — wall-clock, for UI elapsed-time
    /// display only. The server's actual eviction logic uses a monotonic
    /// `Instant` internally, so these timestamps don't have to be precise.
    pub started_at_ms: i64,
    pub last_update_ms: i64,
}

/// Aggregate progress feed served from `POST /api/rpc/worker_status`.
///
/// Two-vec split mirrors the F0.5 design doc shape: callers can short-circuit
/// the "do I need a fade timer?" check by inspecting `recent_complete.is_empty()`
/// without scanning `active`. Both vecs are sorted by `task_id` for stable
/// list rendering across polls.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct WorkerStatus {
    pub active: Vec<TaskProgress>,
    pub recent_complete: Vec<TaskProgress>,
}

impl WorkerStatus {
    pub fn is_empty(&self) -> bool {
        self.active.is_empty() && self.recent_complete.is_empty()
    }
}

#[cfg(test)]
mod metadata_overrides_tests {
    use super::*;

    fn contributor(name: &str) -> Contributor {
        Contributor {
            name: name.to_string(),
            ..Default::default()
        }
    }

    // --- validate() --------------------------------------------------------

    #[test]
    fn validate_accepts_well_formed_override() {
        let ov = MetadataOverrides {
            title: Some("A Reasonable Title".into()),
            description: Some("A short blurb.".into()),
            publisher: Some("Some Press".into()),
            published: Some("2020".into()),
            language: Some("en".into()),
            series: Some("Some Series".into()),
            series_index: Some("1".into()),
            creators: Some(vec![contributor("Ada Lovelace")]),
            subjects: Some(vec!["fiction".into(), "history".into()]),
        };
        assert_eq!(ov.validate(), Ok(()));
    }

    #[test]
    fn validate_accepts_empty_default_override() {
        // An all-`None` override (nothing being overridden) is trivially valid.
        assert_eq!(MetadataOverrides::default().validate(), Ok(()));
    }

    #[test]
    fn validate_rejects_title_exceeding_length_cap() {
        let ov = MetadataOverrides {
            title: Some("x".repeat(MetadataOverrides::MAX_SCALAR_LEN + 1)),
            ..Default::default()
        };
        let err = ov
            .validate()
            .expect_err("over-length title should be rejected");
        assert!(
            err.contains("title"),
            "message should name the field: {err}"
        );
        assert!(err.contains("4096"), "message should name the cap: {err}");
    }

    #[test]
    fn validate_rejects_title_at_length_cap_is_accepted() {
        // Boundary: exactly MAX_SCALAR_LEN is allowed (the check is `> max`).
        let ov = MetadataOverrides {
            title: Some("x".repeat(MetadataOverrides::MAX_SCALAR_LEN)),
            ..Default::default()
        };
        assert_eq!(ov.validate(), Ok(()));
    }

    #[test]
    fn validate_rejects_too_many_subjects() {
        let ov = MetadataOverrides {
            subjects: Some(vec!["tag".to_string(); MetadataOverrides::MAX_SUBJECTS + 1]),
            ..Default::default()
        };
        let err = ov
            .validate()
            .expect_err("over-cap subject count should be rejected");
        assert!(err.contains("too many tags"), "unexpected message: {err}");
    }

    #[test]
    fn validate_rejects_over_long_creator_name() {
        let ov = MetadataOverrides {
            creators: Some(vec![contributor(
                &"x".repeat(MetadataOverrides::MAX_CREATOR_LEN + 1),
            )]),
            ..Default::default()
        };
        let err = ov
            .validate()
            .expect_err("over-length creator name should be rejected");
        assert!(err.contains("creator name"), "unexpected message: {err}");
    }

    // --- merge() -----------------------------------------------------------
    //
    // NOTE on contract: the issue describes `merge` as "applies an override
    // layer onto a base `EbookMetadata`, with empty-array meaning clear-all".
    // The actual `MetadataOverrides::merge` merges two `MetadataOverrides`
    // layers (a prior stored override + an incoming edit), not an
    // `EbookMetadata`, and uses `incoming.field.or(self.field)`. The
    // empty-array-clears-the-base-book behaviour lives in
    // `db::metadata_overrides::apply_overrides`, where `Some(vec![])`
    // overwrites the book's list with an empty vec. The tests below assert the
    // ACTUAL `merge` semantics; case (4) documents that for `merge` itself an
    // empty `Some(vec![])` is an *override layer value* that wins over the
    // base layer (it does not collapse to "don't touch"), which is consistent
    // with — and feeds into — the clear-all behaviour at apply time.

    #[test]
    fn merge_empty_creators_layer_wins_over_base_creators() {
        // Issue case (4): incoming `creators: Some(vec![])` is preserved on the
        // merged override (it does NOT fall back to the base layer's creators).
        // Downstream `apply_overrides` then clears the book's creators.
        let base = MetadataOverrides {
            creators: Some(vec![contributor("Stale Author")]),
            ..Default::default()
        };
        let incoming = MetadataOverrides {
            creators: Some(vec![]),
            ..Default::default()
        };
        let merged = base.merge(&incoming);
        assert_eq!(merged.creators, Some(vec![]));
    }

    #[test]
    fn merge_none_creators_leaves_base_creators_unchanged() {
        // Issue case (5): incoming `creators: None` preserves the base layer.
        let base = MetadataOverrides {
            creators: Some(vec![contributor("Original Author")]),
            ..Default::default()
        };
        let incoming = MetadataOverrides::default();
        let merged = base.merge(&incoming);
        assert_eq!(merged.creators, Some(vec![contributor("Original Author")]));
    }

    #[test]
    fn merge_nonempty_creators_replaces_base_entirely() {
        // Issue case (6): incoming non-empty creators replaces, not appends.
        let base = MetadataOverrides {
            creators: Some(vec![contributor("Old A"), contributor("Old B")]),
            ..Default::default()
        };
        let incoming = MetadataOverrides {
            creators: Some(vec![contributor("New One")]),
            ..Default::default()
        };
        let merged = base.merge(&incoming);
        assert_eq!(merged.creators, Some(vec![contributor("New One")]));
    }

    #[test]
    fn merge_empty_subjects_layer_wins_over_base_subjects() {
        // Adjacent case: same empty-vec-wins semantics for the subjects field.
        let base = MetadataOverrides {
            subjects: Some(vec!["stale".into()]),
            ..Default::default()
        };
        let incoming = MetadataOverrides {
            subjects: Some(vec![]),
            ..Default::default()
        };
        let merged = base.merge(&incoming);
        assert_eq!(merged.subjects, Some(vec![]));
    }

    #[test]
    fn merge_preserves_untouched_scalar_fields_from_base() {
        // An incoming edit that only sets `title` must preserve all other
        // prior-override fields (the documented reason `merge` exists).
        let base = MetadataOverrides {
            title: Some("Old Title".into()),
            publisher: Some("Kept Publisher".into()),
            series: Some("Kept Series".into()),
            ..Default::default()
        };
        let incoming = MetadataOverrides {
            title: Some("New Title".into()),
            ..Default::default()
        };
        let merged = base.merge(&incoming);
        assert_eq!(merged.title, Some("New Title".into()));
        assert_eq!(merged.publisher, Some("Kept Publisher".into()));
        assert_eq!(merged.series, Some("Kept Series".into()));
    }
}
