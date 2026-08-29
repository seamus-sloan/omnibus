//! The book-content read tool family: list an EPUB's chapters, read one
//! chapter's plain text in bounded slices, and full-text search the
//! library's indexed chapter text. Pure reads over existing `GET` endpoints
//! — nothing here touches the [`crate::client::WRITE_ALLOWLIST`].

use rmcp::handler::server::wrapper::Parameters;
use rmcp::{tool, tool_router, ErrorData, Json};
use schemars::JsonSchema;
use serde::Deserialize;

use omnibus_shared::{ChapterListResponse, ChapterTextResponse, ContentSearchResults};

use crate::server::OmnibusMcp;

/// Parameters for the chapter listing.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListChaptersParams {
    /// The book's uuid (the `unique_identifier` field on book records).
    pub book_uuid: String,
}

/// Parameters for the bounded chapter-text read.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ChapterTextParams {
    /// The book's uuid.
    pub book_uuid: String,
    /// The chapter's spine index, from list_chapters (valid indexes are
    /// `0..spine_count`).
    pub spine_index: u64,
    /// Char offset to start from — pass a previous slice's `next_offset`
    /// to continue. Defaults to 0.
    pub offset: Option<u64>,
    /// Slice size in chars; server-clamped to at most 100000.
    pub limit: Option<u64>,
}

/// Parameters for the content search.
#[derive(Debug, Deserialize, JsonSchema)]
pub struct ContentSearchParams {
    /// Full-text query over book content (FTS5 syntax; a plain phrase works).
    pub query: String,
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
        description = "Read one chapter of a book as plain text, in bounded slices (at most 100000 chars per call). Address the chapter by the spine_index from list_chapters. When truncated is true the slice ended before the chapter did — page through by re-calling with offset set to the returned next_offset until truncated is false. has_text: false means the book has no extractable text. Errors if the uuid is unknown or spine_index is out of range."
    )]
    pub async fn read_chapter_text(
        &self,
        Parameters(p): Parameters<ChapterTextParams>,
    ) -> Result<Json<ChapterTextResponse>, ErrorData> {
        let uuid = crate::tools::path_segment(&p.book_uuid, "book_uuid")?;
        let spine_index = p.spine_index;
        let path = format!("/api/ebooks/{uuid}/chapters/{spine_index}/text");
        let mut query: Vec<(&str, String)> = Vec::new();
        if let Some(offset) = p.offset {
            query.push(("offset", offset.to_string()));
        }
        if let Some(limit) = p.limit {
            query.push(("limit", limit.to_string()));
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
        description = "Full-text search over the TEXT of the library's books — distinct from search_books, which matches metadata (title, author, series, tags) only. Use this for \"find the passage where …\" questions. Each hit cites the book (book_uuid, title) and the chapter it came from (spine_index) plus a snippet with the matched terms bracketed; follow up with read_chapter_text on the hit's book_uuid + spine_index to read the surrounding text. Only books with extractable text are indexed, so an empty result does not prove the phrase is absent from unindexed formats."
    )]
    pub async fn search_book_content(
        &self,
        Parameters(p): Parameters<ContentSearchParams>,
    ) -> Result<Json<ContentSearchResults>, ErrorData> {
        let hits: ContentSearchResults = self
            .client
            .get_json("/api/search/content", &[("q", p.query)])
            .await?;
        Ok(Json(hits))
    }
}

#[cfg(test)]
mod tests;
