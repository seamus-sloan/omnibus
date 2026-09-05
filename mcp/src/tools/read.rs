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
    JournalEntry, LibraryContents, PhysicalCopy, ProgressFormat, ProgressRecord, ReadStatusRecord,
    ResumePoint, SeriesDetail, SeriesSummary, SessionLogPage, Shelf, ShelfSummary, SortDir,
    SortKey, StatsRange, StatsSummary, TagWeight,
};

use crate::server::OmnibusMcp;

pub mod views;

#[cfg(test)]
mod tests;

use views::{
    BookmarkView, HighlightView, JournalEntryView, PhysicalCopyView, ProgressRecordView,
    ReadStatusView, ResumePointView, SessionLogEntryView, SessionLogPageView,
};

/// A single book handle, as returned in `unique_identifier` by the listing
/// and search tools.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct BookRef {
    /// The book's uuid (the `unique_identifier` field on book records).
    pub uuid: String,
}

/// Per-reader state `get_book` can fold into its answer, so "tell me about
/// this book for this reader" is one call rather than five.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum BookInclude {
    /// Saved positions, one per format the reader has opened.
    Progress,
    /// want_to_read / reading / finished.
    ReadStatus,
    /// Kept lines with their notes.
    Highlights,
    /// Saved places.
    Bookmarks,
    /// The most recent recorded sittings on this book.
    Sessions,
    /// Physical copies on the shelf (library-wide, not per-reader).
    Copies,
}

/// Parameters for the single-book read.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetBookParams {
    /// The book's uuid (the `unique_identifier` field on book records).
    pub uuid: String,
    /// Which per-reader sections to fold in. Omit for metadata alone.
    #[serde(default)]
    pub include: Option<Vec<BookInclude>>,
}

/// One book's metadata plus whichever per-reader sections the caller asked
/// for. A section is absent when it was not requested; a requested section
/// that has nothing to report is present and empty (or `null`), so "not
/// asked" and "nothing there" stay distinguishable.
#[derive(Debug, Serialize, JsonSchema)]
pub struct BookDetail {
    pub book: EbookMetadata,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<Vec<ProgressRecordView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub read_status: Option<Option<ReadStatusView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub highlights: Option<Vec<HighlightView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bookmarks: Option<Vec<BookmarkView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sessions: Option<Vec<SessionLogEntryView>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub copies: Option<Vec<PhysicalCopyView>>,
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

/// How much of each book a feed entry carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Verbosity {
    /// Enough to name the book and fetch the rest: uuid, title, creators,
    /// cover_url, formats, series.
    #[default]
    Stub,
    /// The whole book record, `get_book`-shaped — description, every
    /// identifier, every on-disk file.
    Full,
}

/// Parameters for the recent-progress feed.
#[derive(Debug, Default, Deserialize, JsonSchema)]
pub struct RecentProgressParams {
    /// How many resume points to return (default 1, server-capped).
    pub limit: Option<i64>,
    /// Book projection per entry; defaults to `stub`. Reach for `full` only
    /// when you need a field the stub omits from every entry — otherwise
    /// call `get_book` on the one book you care about.
    pub verbosity: Option<Verbosity>,
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
        description = "Fetch one book's full metadata by uuid, including its on-disk files (book_files) with per-file formats and sizes. Pass include to fold this reader's state into the same answer — any of progress, read_status, highlights, bookmarks, sessions, copies — instead of making a call per section. Every timestamp comes back as an ISO 8601 string with its unix-seconds twin alongside it under the same name plus _epoch."
    )]
    pub async fn get_book(
        &self,
        Parameters(p): Parameters<GetBookParams>,
    ) -> Result<Json<BookDetail>, ErrorData> {
        let path = format!("/api/ebooks/{}", p.uuid);
        let book: Option<EbookMetadata> = self.client.get_json_opt(&path, &[]).await?;
        let book = book.ok_or_else(|| not_found("book"))?;

        let include = p.include.unwrap_or_default();
        let mut detail = BookDetail {
            book,
            progress: None,
            read_status: None,
            highlights: None,
            bookmarks: None,
            sessions: None,
            copies: None,
        };

        if include.contains(&BookInclude::Progress) {
            detail.progress = Some(self.progress_both_formats(&p.uuid).await?);
        }
        if include.contains(&BookInclude::ReadStatus) {
            let path = format!("/api/read-status/{}", p.uuid);
            let record: Option<ReadStatusRecord> = self.client.get_json(&path, &[]).await?;
            detail.read_status = Some(record.map(Into::into));
        }
        if include.contains(&BookInclude::Highlights) {
            let path = format!("/api/highlights/book/{}", p.uuid);
            let rows: Vec<Highlight> = self.client.get_json(&path, &[]).await?;
            detail.highlights = Some(rows.into_iter().map(Into::into).collect());
        }
        if include.contains(&BookInclude::Bookmarks) {
            let path = format!("/api/bookmarks/book/{}", p.uuid);
            let rows: Vec<Bookmark> = self.client.get_json(&path, &[]).await?;
            detail.bookmarks = Some(rows.into_iter().map(Into::into).collect());
        }
        if include.contains(&BookInclude::Sessions) {
            let page: SessionLogPage = self
                .client
                .get_json("/api/stats/sessions", &[("book", p.uuid.clone())])
                .await?;
            detail.sessions = Some(page.entries.into_iter().map(Into::into).collect());
        }
        if include.contains(&BookInclude::Copies) {
            let path = format!("/api/physical/{}/copies", p.uuid);
            let rows: Vec<PhysicalCopy> = self.client.get_json(&path, &[]).await?;
            detail.copies = Some(rows.into_iter().map(Into::into).collect());
        }
        Ok(Json(detail))
    }

    /// Both formats' saved positions for one book, skipping the formats the
    /// reader has never opened. Two reads because the endpoint answers for
    /// one format at a time, and a book can hold a position in each.
    async fn progress_both_formats(
        &self,
        uuid: &str,
    ) -> Result<Vec<ProgressRecordView>, ErrorData> {
        let path = format!("/api/progress/{uuid}");
        let mut out = Vec::new();
        for format in ["epub", "audio"] {
            let record: Option<ProgressRecord> = self
                .client
                .get_json(&path, &[("format", format.to_string())])
                .await?;
            if let Some(record) = record {
                out.push(record.into());
            }
        }
        Ok(out)
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
        description = "The signed-in user's reading/listening stats over a window (week, month, year, all_time): totals, streaks, per-day activity, top books/authors, superlatives, and goal progress. Day-granularity fields (as_of_day, busiest_week_start, every heatmap day) are already YYYY-MM-DD; the one exception is finished_books[].finished_at, which is unix seconds."
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
        description = "The signed-in user's reading-session log, newest first — one entry per recorded sitting with book, format, and duration. Paginate by echoing next_before back as before; optionally scope to one book uuid. A sitting's format is reading | listening | mixed, which is deliberately wider than the epub | audio a progress record carries: a sitting can span both formats, a saved position cannot. The mapping is reading=epub, listening=audio, and mixed=both in one sitting. started_at and ended_at are ISO 8601, with unix seconds alongside under started_at_epoch / ended_at_epoch; seconds is time actually recorded, not ended_at minus started_at."
    )]
    pub async fn reading_sessions(
        &self,
        Parameters(p): Parameters<SessionLogParams>,
    ) -> Result<Json<SessionLogPageView>, ErrorData> {
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
        let page: SessionLogPage = self.client.get_json("/api/stats/sessions", &query).await?;
        Ok(Json(page.into()))
    }

    #[tool(
        description = "The signed-in user's most recent in-progress books — the 'pick up where you left off' feed, with per-book position and format. Each entry carries a stub of its book by default (uuid, title, creators, cover_url, formats, series); call get_book on a uuid for the rest, or pass verbosity: \"full\" to inline every book record. A progress record's format is epub | audio — narrower than a reading session's reading | listening | mixed, because a saved position belongs to one format. Timestamps are ISO 8601 with unix seconds alongside under the same name plus _epoch."
    )]
    pub async fn recent_progress(
        &self,
        Parameters(p): Parameters<RecentProgressParams>,
    ) -> Result<Json<Vec<ResumePointView>>, ErrorData> {
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(limit) = p.limit {
            query.push(("limit", limit.to_string()));
        }
        let points: Vec<ResumePoint> = self.client.get_json("/api/progress/recent", &query).await?;
        let full = p.verbosity.unwrap_or_default() == Verbosity::Full;
        Ok(Json(
            points
                .into_iter()
                .map(|point| ResumePointView::project(point, full))
                .collect(),
        ))
    }

    #[tool(
        description = "The signed-in user's saved position in one book — EPUB CFI or audio seconds depending on format. Returns null when the user has not opened the book in that format. format is epub | audio; a reading session's wider reading | listening | mixed vocabulary maps onto it as reading=epub and listening=audio, with mixed having no single-format equivalent. updated_at and client_updated_at are ISO 8601, with unix seconds alongside under the same name plus _epoch. Prefer get_book with include for more than one section of a book's reader state."
    )]
    pub async fn book_progress(
        &self,
        Parameters(p): Parameters<BookProgressParams>,
    ) -> Result<Json<Option<ProgressRecordView>>, ErrorData> {
        // Exhaustive match rather than a serde round-trip: a new variant
        // fails the build here instead of silently querying the default.
        let format = match p.format.unwrap_or(ProgressFormat::Epub) {
            ProgressFormat::Epub => "epub",
            ProgressFormat::Audio => "audio",
        };
        let path = format!("/api/progress/{}", p.uuid);
        let record: Option<ProgressRecord> = self
            .client
            .get_json(&path, &[("format", format.to_string())])
            .await?;
        Ok(Json(record.map(Into::into)))
    }

    #[tool(
        description = "The signed-in user's read state for one book (want_to_read / reading / finished, with rating context). Returns null when the book has no state yet — treat that as unread. updated_at and finished_at are ISO 8601, with unix seconds alongside under the same name plus _epoch."
    )]
    pub async fn book_read_status(
        &self,
        Parameters(p): Parameters<BookRef>,
    ) -> Result<Json<Option<ReadStatusView>>, ErrorData> {
        let path = format!("/api/read-status/{}", p.uuid);
        let record: Option<ReadStatusRecord> = self.client.get_json(&path, &[]).await?;
        Ok(Json(record.map(Into::into)))
    }

    #[tool(
        description = "The signed-in user's highlights in one book: highlighted text with color, optional note, and EPUB CFI location. created_at is ISO 8601, with unix seconds alongside under created_at_epoch."
    )]
    pub async fn book_highlights(
        &self,
        Parameters(p): Parameters<BookRef>,
    ) -> Result<Json<Vec<HighlightView>>, ErrorData> {
        let path = format!("/api/highlights/book/{}", p.uuid);
        let rows: Vec<Highlight> = self.client.get_json(&path, &[]).await?;
        Ok(Json(rows.into_iter().map(Into::into).collect()))
    }

    #[tool(
        description = "The signed-in user's bookmarks in one book — reader positions (EPUB CFI) or audiobook timestamps (seconds). created_at is ISO 8601, with unix seconds alongside under created_at_epoch."
    )]
    pub async fn book_bookmarks(
        &self,
        Parameters(p): Parameters<BookRef>,
    ) -> Result<Json<Vec<BookmarkView>>, ErrorData> {
        let path = format!("/api/bookmarks/book/{}", p.uuid);
        let rows: Vec<Bookmark> = self.client.get_json(&path, &[]).await?;
        Ok(Json(rows.into_iter().map(Into::into).collect()))
    }

    #[tool(
        description = "Journal entries for one book, newest first: every user's published entries plus the signed-in user's own drafts, with rendered HTML bodies. created_at and updated_at are ISO 8601, with unix seconds alongside under the same name plus _epoch."
    )]
    pub async fn book_journal_entries(
        &self,
        Parameters(p): Parameters<BookRef>,
    ) -> Result<Json<Vec<JournalEntryView>>, ErrorData> {
        let path = format!("/api/journals/book/{}", p.uuid);
        let rows: Vec<JournalEntry> = self.client.get_json(&path, &[]).await?;
        Ok(Json(rows.into_iter().map(Into::into).collect()))
    }
}
