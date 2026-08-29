//! The read-only tool family: every tool wraps one existing `GET /api/*`
//! endpoint and deserializes into the matching `omnibus_shared` wire type,
//! so a server-side shape change fails loudly here instead of drifting.
//! Descriptions are the model-facing API docs — keep them accurate.

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData, Json};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use omnibus_shared::{
    AuthorDetail, AuthorSummary, Bookmark, EbookLibrary, EbookMetadata, GenreWeight, Highlight,
    JournalEntry, LibraryContents, ProgressFormat, ProgressRecord, ReadStatusRecord, ResumePoint,
    SeriesDetail, SeriesSummary, SessionLogPage, Shelf, ShelfSummary, SortDir, SortKey, StatsRange,
    StatsSummary, TagWeight,
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

/// Parameters for the recent-progress feed.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RecentProgressParams {
    /// How many resume points to return (default 1, server-capped).
    pub limit: Option<i64>,
}

/// Parameters for a single book's progress read.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BookProgressParams {
    /// The book's uuid.
    pub uuid: String,
    /// Which format's position to read; defaults to `epub`.
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
        description = "List the books in the library with full metadata (title, creators, series, subjects, genres, identifiers, formats). With no parameters returns the whole (capped) library; pass sort+dir+limit to paginate and feed next_cursor back for the following page. Book records carry the uuid handle (unique_identifier) the per-book tools take."
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
        description = "Fetch one book's full metadata by uuid, including its on-disk files (book_files) with per-file formats and sizes."
    )]
    pub async fn get_book(
        &self,
        Parameters(p): Parameters<BookRef>,
    ) -> Result<Json<EbookMetadata>, ErrorData> {
        let path = format!("/api/ebooks/{}", p.uuid);
        let book: Option<EbookMetadata> = self.client.get_json_opt(&path, &[]).await?;
        book.map(Json).ok_or_else(|| not_found("book"))
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
        description = "The signed-in user's reading-session log, newest first — one entry per recorded sitting with book, format, and duration. Paginate by echoing next_before back as before; optionally scope to one book uuid."
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
        description = "The signed-in user's most recent in-progress books — the 'pick up where you left off' feed, with per-book position and format."
    )]
    pub async fn recent_progress(
        &self,
        Parameters(p): Parameters<RecentProgressParams>,
    ) -> Result<Json<Vec<ResumePoint>>, ErrorData> {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(limit) = p.limit {
            query.push(("limit", limit.to_string()));
        }
        Ok(Json(
            self.client.get_json("/api/progress/recent", &query).await?,
        ))
    }

    #[tool(
        description = "The signed-in user's saved position in one book — EPUB CFI or audio seconds depending on format. Returns null when the user has not opened the book in that format."
    )]
    pub async fn book_progress(
        &self,
        Parameters(p): Parameters<BookProgressParams>,
    ) -> Result<Json<Option<ProgressRecord>>, ErrorData> {
        // Exhaustive match rather than a serde round-trip: a new variant
        // fails the build here instead of silently querying the default.
        let format = match p.format.unwrap_or(ProgressFormat::Epub) {
            ProgressFormat::Epub => "epub",
            ProgressFormat::Audio => "audio",
        };
        let path = format!("/api/progress/{}", p.uuid);
        Ok(Json(
            self.client
                .get_json(&path, &[("format", format.to_string())])
                .await?,
        ))
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
        description = "The signed-in user's highlights in one book: highlighted text with color, optional note, and EPUB CFI location."
    )]
    pub async fn book_highlights(
        &self,
        Parameters(p): Parameters<BookRef>,
    ) -> Result<Json<Vec<Highlight>>, ErrorData> {
        let path = format!("/api/highlights/book/{}", p.uuid);
        Ok(Json(self.client.get_json(&path, &[]).await?))
    }

    #[tool(
        description = "The signed-in user's bookmarks in one book — reader positions (EPUB CFI) or audiobook timestamps (seconds)."
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
