//! Discovery / browse / search-palette wire types.
//!
//! Covers the author / series / tag detail pages, the author and series
//! browse indexes, the library landing payload, and the command-palette
//! grouped result shape. Each card-level type stays slim so list endpoints
//! avoid the N+1 cost of returning a full `EbookMetadata`.

use serde::{Deserialize, Serialize};

use crate::ebook::EbookMetadata;

/// Response payload for `GET /api/ebooks` and `rpc_get_ebooks`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct EbookLibrary {
    pub path: Option<String>,
    pub books: Vec<EbookMetadata>,
    pub error: Option<String>,
    /// Total FTS5 hit count *before* the server-side `MAX_BOOKS_RETURNED` cap,
    /// set by the search paths so the web client can show "N of M results"
    /// even when `books` is truncated. `None` for the full-library
    /// (`/api/ebooks`, `rpc_get_ebooks`) responses, which surface truncation
    /// via the `X-Total-Count` header instead.
    #[serde(default)]
    pub total: Option<i64>,
}

/// One facet value paired with the number of books carrying it. `value` is the
/// canonical key the filter chips toggle (author/series/tag name, or a
/// lowercased format key like `"epub"`); `count` is the book tally across the
/// whole (unfiltered) library.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FacetCount {
    pub value: String,
    pub count: i64,
}

/// Per-facet book counts for the landing sidebar, computed server-side over
/// the full library so the counts stay correct under keyset pagination (the
/// client only ever holds one page). Each list is ordered by count descending,
/// then value ascending — the order the sidebar renders. Formats are keyed by
/// their lowercased extension (`"epub"`, `"m4b"`).
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct FacetCounts {
    pub authors: Vec<FacetCount>,
    pub series: Vec<FacetCount>,
    pub formats: Vec<FacetCount>,
    pub tags: Vec<FacetCount>,
}

/// Response payload for the F5b keyset-paginated **web** landing read
/// (`rpc_get_ebooks_page`). One page of books plus the opaque cursor to fetch
/// the next page (`None` at end of stream).
///
/// `total` (full unfiltered library count) and `facets` (sidebar counts) are
/// populated only on the **first** page — a cursor-less request — so the
/// client sizes the header and renders the sidebar once instead of recomputing
/// them on every scroll. The cursor is opaque: the client stores `next_cursor`
/// and hands it back verbatim; it never inspects it.
///
/// The mobile REST `GET /api/ebooks` paginated form does **not** use this type:
/// it keeps the `EbookLibrary` body and carries the cursor in the
/// `X-Next-Cursor` header (and has no `facets`), so older clients stay
/// byte-compatible.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryPage {
    pub path: Option<String>,
    pub books: Vec<EbookMetadata>,
    #[serde(default)]
    pub next_cursor: Option<String>,
    #[serde(default)]
    pub total: Option<i64>,
    #[serde(default)]
    pub facets: Option<FacetCounts>,
}

/// Author detail payload for `GET /api/authors/:id` and `rpc_get_author`.
/// Contains the author row plus every book by that author.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct AuthorDetail {
    pub id: i64,
    pub name: String,
    pub sort: Option<String>,
    pub book_count: usize,
    pub books: Vec<EbookMetadata>,
    /// `true` when a usable profile photo is cached for this author
    /// (a `manual` or `openlibrary` row in `author_photos`). The frontend
    /// hero swaps the letter avatar for `<img src="/api/authors/:id/photo">`
    /// when set. `'letter'` negative-cache rows do not set this flag.
    #[serde(default)]
    pub has_photo: bool,
}

/// Result of an admin-triggered "Scan for picture" run. The endpoint
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

/// Lightweight author row for the `/authors` index. Carries only what the
/// card needs — no joined book list. The detail page (`AuthorDetail`) is
/// fetched on click.
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
    /// `true` when a usable profile photo is cached for this author (a
    /// `manual` or `openlibrary` row in `author_photos`). Same semantics
    /// as `AuthorDetail::has_photo` — `'letter'` negative-cache rows do
    /// not set this flag. Lets the `/authors` index render a real `<img>`
    /// in one round trip instead of fetching the detail payload per card.
    #[serde(default)]
    pub has_photo: bool,
}

/// Lightweight series row for the `/series` index. Carries the primary
/// author of the series (first book's first creator) so the card can
/// render the by-line without a second fetch.
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

/// Search-palette response — grouped results with server-side timing.
///
/// Each category is capped at 5 hits server-side. The slim per-hit types
/// (`PaletteBookHit` etc.) carry only display data — no N+1 joins for
/// description / identifiers / subjects.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PaletteResults {
    pub query: String,
    pub books: Vec<PaletteBookHit>,
    pub authors: Vec<PaletteAuthorHit>,
    pub series: Vec<PaletteSeriesHit>,
    pub tags: Vec<PaletteTagHit>,
    pub duration_ms: u64,
    /// True match counts per category, before the 5-hit display cap.
    // `#[serde(default)]` keeps older/partial payloads (and the command
    // palette, which ignores these) deserializing.
    #[serde(default)]
    pub book_total: u32,
    #[serde(default)]
    pub author_total: u32,
    #[serde(default)]
    pub series_total: u32,
    #[serde(default)]
    pub tag_total: u32,
}

impl PaletteResults {
    /// Total number of matches across every result category, using the true
    /// per-category totals (not the capped `Vec` lengths).
    pub fn total_count(&self) -> usize {
        (self.book_total + self.author_total + self.series_total + self.tag_total) as usize
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
    /// Title of this author's first book in the library (by sort order),
    /// used for the "incl. <title>" line on the results page. `None` when
    /// the author has no resolvable book title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_book_title: Option<String>,
}

/// Series hit for the search palette.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PaletteSeriesHit {
    pub id: i64,
    pub name: String,
    pub book_count: u32,
    /// Primary author of the first book in the series, if any.
    pub author_display: Option<String>,
    /// Title of the first book in the series (by sort order), used for the
    /// "incl. <title>" line on the results page. `None` when the series has
    /// no resolvable book title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lead_book_title: Option<String>,
}

/// Tag hit for the search palette.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct PaletteTagHit {
    pub id: i64,
    pub name: String,
    /// Number of books with this tag in the active library.
    pub book_count: u32,
}
