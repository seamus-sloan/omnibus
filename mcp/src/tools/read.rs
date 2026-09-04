//! The read-only tool family: every tool wraps one existing `GET /api/*`
//! endpoint and deserializes into the matching `omnibus_shared` wire type,
//! so a server-side shape change fails loudly here instead of drifting.
//! Descriptions are the model-facing API docs — keep them accurate.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use omnibus_shared::{
    cross_format::CrossFormatCandidate, AuthorDetail, AuthorSummary, BookProgress, Bookmark,
    Contributor, EbookLibrary, EbookMetadata, GenreWeight, Highlight, JournalEntry,
    LibraryContents, PhysicalCopy, ProgressFormat, ProgressRecord, ReadStatusRecord,
    ResolvedPosition, ResumePoint, SeriesDetail, SeriesSummary, SessionLogPage, Shelf,
    ShelfSummary, SortDir, SortKey, StatsRange, StatsSummary, TagWeight,
};

use crate::server::OmnibusMcp;

/// A single book handle, as returned in `unique_identifier` by the listing
/// and search tools.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BookRef {
    /// The book's uuid (the `unique_identifier` field on book records).
    pub uuid: String,
}

/// A numeric id handle for authors, series, and shelves.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct IdRef {
    /// The numeric `id` from the matching list tool.
    pub id: i64,
}

/// Parameters for the paginated book listing.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct ListBooksParams {
    /// Sort axis. Omit everything for the full (capped) library.
    pub sort: Option<SortKey>,
    /// Sort direction; required alongside `sort` when paginating.
    pub dir: Option<SortDir>,
    /// Page size.
    pub limit: Option<i64>,
    /// Opaque cursor from a previous result's `next_cursor`. Must be sent
    /// with the same `sort` and `dir` that produced it.
    pub cursor: Option<String>,
    /// Comma-separated lowercase format filter, e.g. `"epub"` or
    /// `"m4b,m4a,mp3"`.
    pub formats: Option<String>,
}

/// Parameters for full-text search.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct SearchParams {
    /// Search query. Matches title, author, series, and other metadata
    /// (not book text).
    pub q: String,
}

/// Parameters for the stats summary.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct StatsParams {
    /// Reporting window; defaults to the current calendar month.
    pub range: Option<StatsRange>,
}

/// Parameters for the reading-session log.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct SessionLogParams {
    /// Scope to one book uuid.
    pub book: Option<String>,
    /// Page size (server-clamped).
    pub limit: Option<i64>,
    /// The previous page's `next_before` cursor, echoed back verbatim.
    pub before: Option<String>,
}

/// How much of each book's metadata a resume entry carries.
#[derive(Debug, Default, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Verbosity {
    /// Just enough to name and render the book. The default: the full
    /// record inlines the description, every identifier and every file row
    /// per entry, so a three-book feed costs a large slice of context to
    /// answer "what am I reading".
    #[default]
    Stub,
    /// The whole `EbookMetadata` record, as `get_book` returns it.
    Full,
}

/// Parameters for the recent-progress feed.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RecentProgressParams {
    /// How many resume points to return (default 1, server-capped).
    pub limit: Option<i64>,
    /// How much book metadata to inline per entry; defaults to `stub`.
    pub verbosity: Option<Verbosity>,
}

/// The stub projection of a book: what a resume card needs, and nothing
/// else. Fetch `get_book` for the rest.
#[derive(Debug, Serialize, JsonSchema)]
pub struct BookStub {
    pub uuid: Option<String>,
    pub title: Option<String>,
    pub creators: Vec<Contributor>,
    pub series: Option<String>,
    pub series_index: Option<String>,
    pub formats: Vec<String>,
    pub cover_url: Option<String>,
}

impl From<&EbookMetadata> for BookStub {
    fn from(book: &EbookMetadata) -> Self {
        Self {
            uuid: book.unique_identifier.clone(),
            title: book.title.clone(),
            creators: book.creators.clone(),
            series: book.series.clone(),
            series_index: book.series_index.clone(),
            formats: book.formats.clone(),
            cover_url: book.cover_url.clone(),
        }
    }
}

/// One entry of the resume feed under `verbosity: "stub"` — the position
/// and its enrichment in full, the book projected down.
///
/// Written out field by field rather than `#[serde(flatten)]`-ing a
/// `ResumePoint`: flattening emits that struct's own `book` alongside this
/// one, so the payload carried both the full record and the stub under a
/// duplicated key — the whole saving, silently lost.
#[derive(Debug, Serialize, JsonSchema)]
pub struct ResumePointStub {
    pub record: ProgressRecord,
    pub book: BookStub,
    pub linked: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cross_format: Option<CrossFormatCandidate>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_duration_seconds: Option<f64>,
    pub resolved: ResolvedPosition,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_part: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_part_count: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub playback_rate: Option<f64>,
}

impl From<ResumePoint> for ResumePointStub {
    fn from(p: ResumePoint) -> Self {
        Self {
            book: BookStub::from(&p.book),
            record: p.record,
            linked: p.linked,
            cross_format: p.cross_format,
            total_duration_seconds: p.total_duration_seconds,
            resolved: p.resolved,
            audio_part: p.audio_part,
            audio_part_count: p.audio_part_count,
            playback_rate: p.playback_rate,
        }
    }
}

/// Either shape of the resume feed, chosen by `verbosity`.
#[derive(Debug, Serialize, JsonSchema)]
#[serde(untagged)]
pub enum RecentProgress {
    Stub(Vec<ResumePointStub>),
    Full(Vec<ResumePoint>),
}

/// Per-user state `get_book` can fold into one answer.
#[derive(Debug, Clone, Copy, Deserialize, Serialize, JsonSchema, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BookInclude {
    Progress,
    ReadStatus,
    Highlights,
    Bookmarks,
    Sessions,
    Copies,
}

/// Parameters for the per-book read.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetBookParams {
    /// The book's uuid (the `unique_identifier` field on book records).
    pub uuid: String,
    /// Per-user state to fold in alongside the metadata, so "tell me about
    /// this book for this reader" is one call rather than five.
    pub include: Option<Vec<BookInclude>>,
}

/// `get_book`'s answer: the metadata, plus whatever `include` asked for.
/// Every extra block is absent unless requested.
#[derive(Debug, Serialize, JsonSchema)]
pub struct BookDetail {
    pub book: EbookMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<BookProgress>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_status: Option<Option<ReadStatusRecord>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlights: Option<Vec<Highlight>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmarks: Option<Vec<Bookmark>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions: Option<SessionLogPage>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copies: Option<Vec<PhysicalCopy>>,
}

/// Parameters for a single book's progress read.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BookProgressParams {
    /// The book's uuid.
    pub uuid: String,
    /// Narrow the answer to one format. Omit it — the default returns
    /// every format the reader has a position in, which is what makes
    /// `furthest` meaningful.
    pub format: Option<ProgressFormat>,
}

/// One page of books plus the pagination metadata the REST endpoint carries
/// in headers (`X-Next-Cursor` / `X-Total-Count`).
#[derive(Debug, Serialize, JsonSchema)]
pub struct BookPage {
    /// The books plus the library path/error context.
    pub library: EbookLibrary,
    /// Cursor for the next page; absent at end of stream or when the request
    /// was unpaginated.
    pub next_cursor: Option<String>,
    /// Total matching books before the server's response cap.
    pub total: Option<i64>,
}

fn not_found(what: &str) -> ErrorData {
    ErrorData::invalid_params(format!("{what} not found"), None)
}

#[tool_router(router = read_tools, vis = "pub(crate)")]
impl OmnibusMcp {
    #[tool(
        description = "Overview of what is on disk: file counts per format for the ebook and audiobook libraries, with each library's configured path. Cheap sanity check that the instance has content; use list_books for the actual catalog."
    )]
    pub async fn library_overview(&self) -> Result<Json<LibraryContents>, ErrorData> {
        Ok(Json(self.client.get_json("/api/library", &[]).await?))
    }

    #[tool(
        description = "List the books in the library with full metadata (title, creators, series, subjects, genres, identifiers, formats). With no parameters returns the whole (capped) library; pass sort+dir+limit to paginate and feed next_cursor back for the following page. Book records carry the uuid handle (unique_identifier) the per-book tools take.\n\nlast_interacted_at is LIBRARY-WIDE and is not a reading timestamp: it is the most recent moment anyone rated the book, published a journal entry on it, changed its read status, edited its metadata or cover, added it to the library, or checked in a physical copy. Reading position is NOT one of those signals, so this field can jump while a reader's position barely moves — and moves for one reader when another acts. For \"when did this reader last read this\", use recent_progress or book_progress."
    )]
    pub async fn list_books(
        &self,
        Parameters(p): Parameters<ListBooksParams>,
    ) -> Result<Json<BookPage>, ErrorData> {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(sort) = p.sort {
            query.push(("sort", sort.as_wire().to_string()));
        }
        if let Some(dir) = p.dir {
            query.push(("dir", dir.as_wire().to_string()));
        }
        if let Some(limit) = p.limit {
            query.push(("limit", limit.to_string()));
        }
        if let Some(cursor) = p.cursor {
            query.push(("cursor", cursor));
        }
        if let Some(formats) = p.formats {
            query.push(("formats", formats));
        }
        let (library, meta) = self
            .client
            .get_json_with_meta("/api/ebooks", &query)
            .await?;
        Ok(Json(BookPage {
            library,
            next_cursor: meta.next_cursor,
            total: meta.total,
        }))
    }

    #[tool(
        description = "Fetch one book's full metadata by uuid, including its on-disk files (book_files) with per-file formats, sizes, and — for audio — duration_seconds, so an audiobook's runtime never has to be sourced out of band.\n\nPass `include` to fold this reader's own state into the same answer instead of making five more calls: \"progress\" (every format's position, enriched), \"read_status\", \"highlights\", \"bookmarks\", \"sessions\", \"copies\". Each requested block appears as a top-level field alongside `book`; unrequested blocks are absent. Use it for \"tell me about this book for this reader\"."
    )]
    pub async fn get_book(
        &self,
        Parameters(p): Parameters<GetBookParams>,
    ) -> Result<Json<BookDetail>, ErrorData> {
        let uuid = crate::tools::path_segment(&p.uuid, "uuid")?;
        let path = format!("/api/ebooks/{uuid}");
        let book: Option<EbookMetadata> = self.client.get_json_opt(&path, &[]).await?;
        let book = book.ok_or_else(|| not_found("book"))?;

        let include = p.include.unwrap_or_default();
        let wants = |what: BookInclude| include.contains(&what);
        let mut detail = BookDetail {
            book,
            progress: None,
            read_status: None,
            highlights: None,
            bookmarks: None,
            sessions: None,
            copies: None,
        };
        // Sequential rather than concurrent: the client serializes on one
        // token anyway, and a handful of small reads is not worth the
        // machinery — the win here is the caller making one tool call, not
        // the wall clock.
        if wants(BookInclude::Progress) {
            detail.progress = Some(
                self.client
                    .get_json(&format!("/api/progress/{uuid}"), &[])
                    .await?,
            );
        }
        if wants(BookInclude::ReadStatus) {
            detail.read_status = Some(
                self.client
                    .get_json(&format!("/api/read-status/{uuid}"), &[])
                    .await?,
            );
        }
        if wants(BookInclude::Highlights) {
            detail.highlights = Some(
                self.client
                    .get_json(&format!("/api/highlights/book/{uuid}"), &[])
                    .await?,
            );
        }
        if wants(BookInclude::Bookmarks) {
            detail.bookmarks = Some(
                self.client
                    .get_json(&format!("/api/bookmarks/book/{uuid}"), &[])
                    .await?,
            );
        }
        if wants(BookInclude::Sessions) {
            detail.sessions = Some(
                self.client
                    .get_json("/api/stats/sessions", &[("book", uuid.to_string())])
                    .await?,
            );
        }
        if wants(BookInclude::Copies) {
            detail.copies = Some(
                self.client
                    .get_json(&format!("/api/physical/{uuid}/copies"), &[])
                    .await?,
            );
        }
        Ok(Json(detail))
    }

    #[tool(
        description = "Full-text search over book metadata (title, author, series, subjects — not book text). Returns matching books ranked by relevance; total is the full hit count before the response cap."
    )]
    pub async fn search_books(
        &self,
        Parameters(p): Parameters<SearchParams>,
    ) -> Result<Json<BookPage>, ErrorData> {
        let (library, meta) = self
            .client
            .get_json_with_meta("/api/search", &[("q", p.q)])
            .await?;
        Ok(Json(BookPage {
            library,
            next_cursor: None,
            total: meta.total,
        }))
    }

    #[tool(
        description = "List every author across both libraries with book counts. Author ids feed get_author."
    )]
    pub async fn list_authors(&self) -> Result<Json<Vec<AuthorSummary>>, ErrorData> {
        Ok(Json(self.client.get_json("/api/authors", &[]).await?))
    }

    #[tool(
        description = "Fetch one author's detail page by id: their books across both libraries plus roles and series involvement."
    )]
    pub async fn get_author(
        &self,
        Parameters(p): Parameters<IdRef>,
    ) -> Result<Json<AuthorDetail>, ErrorData> {
        let path = format!("/api/authors/{}", p.id);
        let author: Option<AuthorDetail> = self.client.get_json_opt(&path, &[]).await?;
        author.map(Json).ok_or_else(|| not_found("author"))
    }

    #[tool(
        description = "List every series across both libraries with book counts and primary author. Series ids feed get_series."
    )]
    pub async fn list_series(&self) -> Result<Json<Vec<SeriesSummary>>, ErrorData> {
        Ok(Json(self.client.get_json("/api/series", &[]).await?))
    }

    #[tool(description = "Fetch one series' detail by id: its books in series order.")]
    pub async fn get_series(
        &self,
        Parameters(p): Parameters<IdRef>,
    ) -> Result<Json<SeriesDetail>, ErrorData> {
        let path = format!("/api/series/{}", p.id);
        let series: Option<SeriesDetail> = self.client.get_json_opt(&path, &[]).await?;
        series.map(Json).ok_or_else(|| not_found("series"))
    }

    #[tool(
        description = "The weighted tag cloud: every subject/tag in the library with how many books carry it."
    )]
    pub async fn list_tags(&self) -> Result<Json<Vec<TagWeight>>, ErrorData> {
        Ok(Json(self.client.get_json("/api/tags", &[]).await?))
    }

    #[tool(
        description = "The weighted genre cloud: every user-assigned genre with how many books carry it. Genres are user-curated (unlike tags, which come from the files)."
    )]
    pub async fn list_genres(&self) -> Result<Json<Vec<GenreWeight>>, ErrorData> {
        Ok(Json(self.client.get_json("/api/genres", &[]).await?))
    }

    #[tool(
        description = "List every shelf visible to the signed-in user, with kind (manual or smart/rule-based), visibility, and live book counts. Shelf ids feed get_shelf."
    )]
    pub async fn list_shelves(&self) -> Result<Json<Vec<ShelfSummary>>, ErrorData> {
        Ok(Json(self.client.get_json("/api/shelves", &[]).await?))
    }

    #[tool(
        description = "Fetch one shelf by id, including its smart-shelf rules (field/op/value with match mode) when it is rule-based."
    )]
    pub async fn get_shelf(
        &self,
        Parameters(p): Parameters<IdRef>,
    ) -> Result<Json<Shelf>, ErrorData> {
        let path = format!("/api/shelves/{}", p.id);
        let shelf: Option<Shelf> = self.client.get_json_opt(&path, &[]).await?;
        shelf.map(Json).ok_or_else(|| not_found("shelf"))
    }

    #[tool(
        description = "Which visible hand-picked shelves contain this book — returns their shelf ids."
    )]
    pub async fn shelves_containing_book(
        &self,
        Parameters(p): Parameters<BookRef>,
    ) -> Result<Json<Vec<i64>>, ErrorData> {
        let path = format!("/api/shelves/containing/{}", p.uuid);
        Ok(Json(self.client.get_json(&path, &[]).await?))
    }

    #[tool(
        description = "The signed-in user's reading/listening stats over a window (week, month, year, all_time): totals, streaks, per-day activity, top books/authors, superlatives, and goal progress."
    )]
    pub async fn reading_stats(
        &self,
        Parameters(p): Parameters<StatsParams>,
    ) -> Result<Json<StatsSummary>, ErrorData> {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(range) = p.range {
            query.push(("range", range.as_query().to_string()));
        }
        Ok(Json(self.client.get_json("/api/stats", &query).await?))
    }

    #[tool(
        description = "The signed-in user's reading-session log, newest first — one entry per recorded sitting with book, format, and duration. Paginate by echoing next_before back as before; optionally scope to one book uuid.\n\nFORMAT VOCABULARY: a session's format is \"reading\" | \"listening\" | \"mixed\", because one sitting can span both. Progress records use \"epub\" | \"audio\" instead, for the single format a position belongs to. The mapping is reading=epub, listening=audio; \"mixed\" has no progress-record equivalent. Every timestamp carries an ISO 8601 sibling (started_at_iso, ended_at_iso) alongside the unix seconds, so no epoch arithmetic is needed."
    )]
    pub async fn reading_sessions(
        &self,
        Parameters(p): Parameters<SessionLogParams>,
    ) -> Result<Json<SessionLogPage>, ErrorData> {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(book) = p.book {
            query.push(("book", book));
        }
        if let Some(limit) = p.limit {
            query.push(("limit", limit.to_string()));
        }
        if let Some(before) = p.before {
            query.push(("before", before));
        }
        Ok(Json(
            self.client.get_json("/api/stats/sessions", &query).await?,
        ))
    }

    #[tool(
        description = "The signed-in user's most recent in-progress books — the 'pick up where you left off' feed, with per-book position and format.\n\nBy default each entry carries a stub of the book (uuid, title, creators, series, formats, cover_url); pass verbosity: \"full\" for the whole metadata record, which inlines the description, every identifier and every file row per entry. Audio entries carry total_duration_seconds and audio_part/audio_part_count — those are CONTAINER PART marks, not book chapters, so a 65-chapter book stored as a 4-part M4B reports part 4 of 4. Book chapters, when known, are on each entry's `resolved` block; call book_progress for a fully resolved position."
    )]
    pub async fn recent_progress(
        &self,
        Parameters(p): Parameters<RecentProgressParams>,
    ) -> Result<Json<RecentProgress>, ErrorData> {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(limit) = p.limit {
            query.push(("limit", limit.to_string()));
        }
        let points: Vec<ResumePoint> = self.client.get_json("/api/progress/recent", &query).await?;
        Ok(Json(match p.verbosity.unwrap_or_default() {
            Verbosity::Full => RecentProgress::Full(points),
            Verbosity::Stub => {
                RecentProgress::Stub(points.into_iter().map(ResumePointStub::from).collect())
            }
        }))
    }

    #[tool(
        description = "Where the signed-in user is in one book. Returns EVERY format they have a position in (not just the ebook), each with the stored position and a `resolved` block naming the chapter, the percent through that chapter, and the percent through the whole book.\n\nRead `furthest` first: it names which format represents the reader's true place, so a reader 87% through the audiobook and 47% through the EPUB reads as 87%, not 47%. Audio records carry total_duration_seconds and a derived progress_percent, so an audiobook's runtime never has to be guessed. `resolved.confidence` is \"exact\", \"approximate\" (derived through a lossy step), or \"unknown\" (nothing was derivable — the other fields are then absent, and MUST NOT be treated as position zero).\n\naudio_part/audio_part_count are container part marks, NOT book chapters; book chapters live on `resolved`. Pass `format` only to narrow deliberately — the default is what makes `furthest` meaningful. `records` is empty when the user has never opened the book."
    )]
    pub async fn book_progress(
        &self,
        Parameters(p): Parameters<BookProgressParams>,
    ) -> Result<Json<BookProgress>, ErrorData> {
        let uuid = crate::tools::path_segment(&p.uuid, "uuid")?;
        let path = format!("/api/progress/{uuid}");
        let mut query: Vec<(&str, String)> = Vec::new();
        // Exhaustive match rather than a serde round-trip: a new variant
        // fails the build here instead of silently querying the wrong one.
        if let Some(format) = p.format {
            query.push((
                "format",
                match format {
                    ProgressFormat::Epub => "epub",
                    ProgressFormat::Audio => "audio",
                }
                .to_string(),
            ));
        }
        Ok(Json(self.client.get_json(&path, &query).await?))
    }

    #[tool(
        description = "The signed-in user's read state for one book (want_to_read / reading / finished, with rating context). Returns null when the book has no state yet — treat that as unread."
    )]
    pub async fn book_read_status(
        &self,
        Parameters(p): Parameters<BookRef>,
    ) -> Result<Json<Option<ReadStatusRecord>>, ErrorData> {
        let path = format!("/api/read-status/{}", p.uuid);
        Ok(Json(self.client.get_json(&path, &[]).await?))
    }

    #[tool(
        description = "The signed-in user's highlights in one book: highlighted text with color, optional note, and EPUB CFI location. Each is placed in the book — spine_index, chapter_title, percent_through_book — and the list is ordered by that position, so a run of highlights reads as a pass through the text. Anchors that cannot be placed (Kobo-origin highlights carry no CFI) report null and sort last. created_at carries an ISO 8601 sibling."
    )]
    pub async fn book_highlights(
        &self,
        Parameters(p): Parameters<BookRef>,
    ) -> Result<Json<Vec<Highlight>>, ErrorData> {
        let path = format!("/api/highlights/book/{}", p.uuid);
        Ok(Json(self.client.get_json(&path, &[]).await?))
    }

    #[tool(
        description = "The signed-in user's bookmarks in one book — reader positions (EPUB CFI) or audiobook timestamps (seconds). Each is placed in the book (spine_index, chapter_title, percent_through_book) and the list is ordered by that position; an audiobook bookmark has no spine_index but is placed by percent all the same. created_at carries an ISO 8601 sibling."
    )]
    pub async fn book_bookmarks(
        &self,
        Parameters(p): Parameters<BookRef>,
    ) -> Result<Json<Vec<Bookmark>>, ErrorData> {
        let path = format!("/api/bookmarks/book/{}", p.uuid);
        Ok(Json(self.client.get_json(&path, &[]).await?))
    }

    #[tool(
        description = "Journal entries for one book, newest first: every user's published entries plus the signed-in user's own drafts, with rendered HTML bodies."
    )]
    pub async fn book_journal_entries(
        &self,
        Parameters(p): Parameters<BookRef>,
    ) -> Result<Json<Vec<JournalEntry>>, ErrorData> {
        let path = format!("/api/journals/book/{}", p.uuid);
        Ok(Json(self.client.get_json(&path, &[]).await?))
    }
}
