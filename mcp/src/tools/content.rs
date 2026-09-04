//! The book-content read tool family: list an EPUB's chapters, read one
//! chapter's plain text in bounded slices, and full-text search the
//! library's indexed chapter text. Pure reads over existing `GET` endpoints
//! — nothing here touches the [`crate::client::WRITE_ALLOWLIST`].

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData, Json};
use schemars::JsonSchema;
use serde::Deserialize;

use omnibus_shared::{
    ChapterListResponse, ChapterTextResponse, ContentSearchResults, SpoilerFilter,
};

use crate::server::OmnibusMcp;

/// Parameters for the chapter listing.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListChaptersParams {
    /// The book's uuid (the `unique_identifier` field on book records).
    pub book_uuid: String,
}

/// Parameters for the bounded chapter-text read. The numeric fields are
/// `i64` to match the shared wire types (`ChapterListEntry::spine_index`,
/// `ChapterTextResponse::next_offset`) so a schema-driven client round-trips
/// them without signed/unsigned coercion; negatives are rejected up front.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChapterTextParams {
    /// The book's uuid.
    pub book_uuid: String,
    /// The chapter's spine index, from list_chapters (valid indexes are
    /// `0..spine_count`).
    pub spine_index: i64,
    /// Char offset to start from — pass a previous slice's `next_offset`
    /// to continue. Defaults to 0.
    pub offset: Option<i64>,
    /// Slice size in chars; server-clamped to at most 100000.
    pub limit: Option<i64>,
    /// Stop the text at the reader's own furthest recorded position in this
    /// book. Use it whenever the reader is mid-book and the conversation
    /// must not run ahead of them.
    pub stop_at_progress: Option<bool>,
}

/// Reject a negative value before it is formatted into a URL the server's
/// unsigned parses would answer with an opaque 400.
fn non_negative(value: i64, what: &str) -> Result<i64, ErrorData> {
    if value < 0 {
        return Err(ErrorData::invalid_params(
            format!("{what} must be non-negative: got {value}"),
            None,
        ));
    }
    Ok(value)
}

/// Parameters for the content search.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ContentSearchParams {
    /// Words that must ALL appear in the same chapter. A natural-language
    /// phrase of several words usually matches nothing — search one
    /// distinctive term first, then narrow. Wrap in double quotes for an
    /// exact phrase; the final word is prefix-matched.
    pub query: String,
    /// Scope to one book's text. Strongly preferred when the question is
    /// about a specific book — without it a term from a series returns
    /// interleaved hits from every volume.
    pub book_uuid: Option<String>,
    /// Scope to several books (a series), as uuids.
    pub book_uuids: Option<Vec<String>>,
    /// Maximum hits to return; server-capped at 50.
    pub limit: Option<i64>,
    /// What to do about hits past the reader's own position: "annotate"
    /// (default) tags each hit with `ahead_of_reader` and
    /// `position_delta_percent`; "exclude" withholds them and reports
    /// `withheld_ahead`; "none" disables the check.
    pub spoiler_filter: Option<SpoilerFilter>,
}

#[tool_router(router = content_tools, vis = "pub(crate)")]
impl OmnibusMcp {
    #[tool(
        description = "List a book's chapters: TOC titles plus the spine_index each chapter's text is read by (via read_chapter_text), and spine_count, the number of addressable spine documents. has_text: false means the book's served format has no extractable text (audiobook-only, comic-only). A TOC-less but readable EPUB reports has_text: true with an empty chapters list — its text is still readable by spine index up to spine_count. Errors if the uuid is unknown."
    )]
    pub async fn list_chapters(
        &self,
        Parameters(p): Parameters<ListChaptersParams>,
    ) -> Result<Json<ChapterListResponse>, ErrorData> {
        let uuid = crate::tools::path_segment(&p.book_uuid, "book_uuid")?;
        let path = format!("/api/ebooks/{uuid}/chapters");
        let chapters: Option<ChapterListResponse> = self.client.get_json_opt(&path, &[]).await?;
        chapters
            .map(Json)
            .ok_or_else(|| ErrorData::invalid_params(format!("book {uuid} not found"), None))
    }

    #[tool(
        description = "Read one chapter of a book as plain text, in bounded slices (at most 100000 chars per call). Address the chapter by the spine_index from list_chapters. When truncated is true the slice ended before the chapter did — page through by re-calling with offset set to the returned next_offset until truncated is false. has_text: false means the book has no extractable text. Errors if the uuid is unknown or spine_index is out of range.\n\nSet stop_at_progress: true when the reader is partway through the book and must not be spoiled: the text is cut at their furthest recorded position and truncated_by_progress comes back true. That cut is deliberate and final — unlike truncated, it carries no next_offset, because paging past it is the thing the flag exists to prevent. A reader whose position cannot be placed gets no text at all rather than the whole chapter."
    )]
    pub async fn read_chapter_text(
        &self,
        Parameters(p): Parameters<ChapterTextParams>,
    ) -> Result<Json<ChapterTextResponse>, ErrorData> {
        let uuid = crate::tools::path_segment(&p.book_uuid, "book_uuid")?;
        let spine_index = non_negative(p.spine_index, "spine_index")?;
        let path = format!("/api/ebooks/{uuid}/chapters/{spine_index}/text");
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(offset) = p.offset {
            query.push(("offset", non_negative(offset, "offset")?.to_string()));
        }
        if let Some(limit) = p.limit {
            query.push(("limit", non_negative(limit, "limit")?.to_string()));
        }
        if p.stop_at_progress.unwrap_or(false) {
            query.push(("stop_at_progress", "true".to_string()));
        }
        let text: Option<ChapterTextResponse> = self.client.get_json_opt(&path, &query).await?;
        text.map(Json).ok_or_else(|| {
            ErrorData::invalid_params(
                format!(
                    "book {uuid} not found, or spine_index {spine_index} is out of range \
                     (list_chapters reports the valid range as 0..spine_count)"
                ),
                None,
            )
        })
    }

    #[tool(
        description = "Full-text search over the TEXT of the library's books — distinct from search_books, which matches metadata (title, author, series, tags) only. Use this for \"find the passage where …\" questions. Each hit cites the book (book_uuid, title), the chapter it came from (spine_index and chapter_title) and a snippet with the matched terms bracketed; follow up with read_chapter_text on the hit's book_uuid + spine_index to read the surrounding text.\n\nQUERY SYNTAX: every term must appear in the SAME chapter. A multi-word natural-language phrase therefore usually returns nothing even when each word is common — search one distinctive term, then narrow. Double quotes make an exact phrase; the last word is prefix-matched. An empty result carries a `hint` when the query form is the likely cause.\n\nPass book_uuid (or book_uuids) to scope to one book or a series — without it a term from a series returns interleaved hits from every volume. spoiler_filter places each hit against the reader's own position: \"annotate\" (the default) tags hits with ahead_of_reader and position_delta_percent, \"exclude\" withholds them and reports withheld_ahead so you can say an answer exists without seeing it. A hit whose position cannot be determined is withheld by \"exclude\" rather than assumed safe, and ahead_of_reader: null means unknown, NOT safe. Only books with extractable text are indexed, so an empty result does not prove the phrase is absent from unindexed formats."
    )]
    pub async fn search_book_content(
        &self,
        Parameters(p): Parameters<ContentSearchParams>,
    ) -> Result<Json<ContentSearchResults>, ErrorData> {
        let mut query: Vec<(&str, String)> = vec![("q", p.query)];
        if let Some(uuid) = p.book_uuid {
            query.push((
                "book_uuid",
                crate::tools::path_segment(&uuid, "book_uuid")?.to_string(),
            ));
        }
        if let Some(uuids) = p.book_uuids {
            if !uuids.is_empty() {
                query.push(("book_uuids", uuids.join(",")));
            }
        }
        if let Some(limit) = p.limit {
            query.push(("limit", non_negative(limit, "limit")?.to_string()));
        }
        if let Some(filter) = p.spoiler_filter {
            query.push((
                "spoiler_filter",
                match filter {
                    SpoilerFilter::None => "none",
                    SpoilerFilter::Annotate => "annotate",
                    SpoilerFilter::Exclude => "exclude",
                }
                .to_string(),
            ));
        }
        let hits: ContentSearchResults =
            self.client.get_json("/api/search/content", &query).await?;
        Ok(Json(hits))
    }
}

#[cfg(test)]
mod tests;
