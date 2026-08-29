use std::sync::Arc;

use axum::extract::{Path, Query};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use rmcp::handler::server::wrapper::Parameters;

use omnibus_shared::{
    ChapterListEntry, ChapterListResponse, ChapterTextResponse, ContentSearchHit,
    ContentSearchResults,
};

use super::*;
use crate::client::OmnibusClient;
use crate::config::Config;

/// Every content tool this issue ships, by MCP tool name.
const EXPECTED_TOOLS: &[&str] = &["list_chapters", "read_chapter_text", "search_book_content"];

async fn login() -> AxumJson<serde_json::Value> {
    AxumJson(serde_json::json!({
        "user": {
            "id": 1, "username": "reader", "is_admin": false,
            "can_upload": false, "can_edit": false, "can_download": true
        },
        "token": "token-1",
    }))
}

async fn chapters(Path(uuid): Path<String>) -> Result<AxumJson<ChapterListResponse>, StatusCode> {
    match uuid.as_str() {
        "uuid-frank" => Ok(AxumJson(ChapterListResponse {
            book_uuid: uuid,
            has_text: true,
            spine_count: 3,
            chapters: vec![ChapterListEntry {
                ordinal: 0,
                title: "Letter 1".into(),
                spine_index: 1,
            }],
        })),
        "uuid-audio" => Ok(AxumJson(ChapterListResponse {
            book_uuid: uuid,
            has_text: false,
            spine_count: 0,
            chapters: vec![],
        })),
        _ => Err(StatusCode::NOT_FOUND),
    }
}

#[derive(serde::Deserialize)]
struct TextQuery {
    offset: Option<i64>,
    limit: Option<i64>,
}

async fn chapter_text(
    Path((uuid, spine_index)): Path<(String, i64)>,
    Query(q): Query<TextQuery>,
) -> Result<AxumJson<ChapterTextResponse>, StatusCode> {
    if uuid != "uuid-frank" || spine_index >= 3 {
        return Err(StatusCode::NOT_FOUND);
    }
    // Echo the requested window as a truncated slice so the test can assert
    // the boundary fields pass through the tool untouched.
    let offset = q.offset.unwrap_or(0);
    let limit = q.limit.unwrap_or(100_000);
    Ok(AxumJson(ChapterTextResponse {
        book_uuid: uuid,
        has_text: true,
        spine_index,
        text: "It was on a dreary night of November".into(),
        offset,
        total_chars: 5_000,
        truncated: true,
        next_offset: Some(offset + limit),
    }))
}

async fn content_search(
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> AxumJson<ContentSearchResults> {
    if q.get("q").map(String::as_str) == Some("dreary night") {
        AxumJson(ContentSearchResults {
            hits: vec![ContentSearchHit {
                book_uuid: "uuid-frank".into(),
                spine_index: 1,
                title: "Frankenstein".into(),
                snippet: "It was on a [dreary] [night] of November…".into(),
            }],
        })
    } else {
        AxumJson(ContentSearchResults::default())
    }
}

/// Boot a stub instance serving the content read routes.
async fn stub_service() -> OmnibusMcp {
    let app = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/ebooks/{uuid}/chapters", get(chapters))
        .route(
            "/api/ebooks/{uuid}/chapters/{spine_index}/text",
            get(chapter_text),
        )
        .route("/api/search/content", get(content_search));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = OmnibusClient::new(Config {
        base_url: format!("http://{addr}"),
        username: "reader".into(),
        password: "correct horse battery".into(),
    })
    .unwrap();
    OmnibusMcp::new(Arc::new(client))
}

/// `unwrap_err` needs `T: Debug`, which `rmcp::Json` does not implement.
fn expect_err<T>(result: Result<T, ErrorData>) -> ErrorData {
    match result {
        Err(e) => e,
        Ok(_) => panic!("expected an error"),
    }
}

#[test]
fn content_tools_router_lists_every_expected_tool_with_a_description() {
    let router = OmnibusMcp::content_tools();
    let tools = router.list_all();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in EXPECTED_TOOLS {
        assert!(names.contains(expected), "missing tool {expected}");
    }
    assert_eq!(
        tools.len(),
        EXPECTED_TOOLS.len(),
        "unexpected extra tools: {names:?}"
    );
    for tool in &tools {
        let desc = tool.description.as_deref().unwrap_or_default();
        assert!(
            desc.len() > 20,
            "tool {} needs a real description",
            tool.name
        );
    }
}

#[tokio::test]
async fn list_chapters_returns_the_toc_with_spine_addressing() {
    let service = stub_service().await;
    let listing = service
        .list_chapters(Parameters(ListChaptersParams {
            book_uuid: "uuid-frank".into(),
        }))
        .await
        .unwrap();
    assert!(listing.0.has_text);
    assert_eq!(listing.0.spine_count, 3);
    assert_eq!(listing.0.chapters.len(), 1);
    assert_eq!(listing.0.chapters[0].title, "Letter 1");
    assert_eq!(listing.0.chapters[0].spine_index, 1);
}

#[tokio::test]
async fn list_chapters_passes_through_the_no_text_answer() {
    let service = stub_service().await;
    let listing = service
        .list_chapters(Parameters(ListChaptersParams {
            book_uuid: "uuid-audio".into(),
        }))
        .await
        .unwrap();
    assert!(!listing.0.has_text);
    assert_eq!(listing.0.spine_count, 0);
    assert!(listing.0.chapters.is_empty());
}

#[tokio::test]
async fn list_chapters_reports_not_found_for_an_unknown_uuid() {
    let service = stub_service().await;
    let err = expect_err(
        service
            .list_chapters(Parameters(ListChaptersParams {
                book_uuid: "uuid-missing".into(),
            }))
            .await,
    );
    assert!(err.message.contains("not found"));
}

#[tokio::test]
async fn read_chapter_text_sends_the_window_and_surfaces_the_truncation_boundary() {
    let service = stub_service().await;
    let slice = service
        .read_chapter_text(Parameters(ChapterTextParams {
            book_uuid: "uuid-frank".into(),
            spine_index: 1,
            offset: Some(200),
            limit: Some(50),
        }))
        .await
        .unwrap();
    assert!(slice.0.has_text);
    assert_eq!(slice.0.spine_index, 1);
    assert!(slice.0.text.contains("dreary night"));
    // The boundary fields the description tells the model to page by.
    assert_eq!(slice.0.offset, 200);
    assert_eq!(slice.0.total_chars, 5_000);
    assert!(slice.0.truncated);
    assert_eq!(slice.0.next_offset, Some(250));
}

#[tokio::test]
async fn read_chapter_text_reports_an_out_of_range_spine_index() {
    let service = stub_service().await;
    let err = expect_err(
        service
            .read_chapter_text(Parameters(ChapterTextParams {
                book_uuid: "uuid-frank".into(),
                spine_index: 9,
                offset: None,
                limit: None,
            }))
            .await,
    );
    assert!(err.message.contains("out of range"), "got: {}", err.message);
    assert!(err.message.contains("spine_count"));
}

#[tokio::test]
async fn search_book_content_returns_chapter_cited_hits() {
    let service = stub_service().await;
    let results = service
        .search_book_content(Parameters(ContentSearchParams {
            query: "dreary night".into(),
        }))
        .await
        .unwrap();
    assert_eq!(results.0.hits.len(), 1);
    let hit = &results.0.hits[0];
    assert_eq!(hit.book_uuid, "uuid-frank");
    assert_eq!(hit.spine_index, 1);
    assert_eq!(hit.title, "Frankenstein");
    assert!(hit.snippet.contains("[dreary]"));
}

#[tokio::test]
async fn search_book_content_answers_no_match_with_an_empty_hit_list() {
    let service = stub_service().await;
    let results = service
        .search_book_content(Parameters(ContentSearchParams {
            query: "phrase in no book".into(),
        }))
        .await
        .unwrap();
    assert!(results.0.hits.is_empty());
}

#[tokio::test]
async fn read_chapter_text_rejects_a_negative_spine_index_locally() {
    // The params are i64 to match the shared wire types; a negative never
    // becomes a URL the server's unsigned parse would opaquely 400.
    let service = stub_service().await;
    let err = expect_err(
        service
            .read_chapter_text(Parameters(ChapterTextParams {
                book_uuid: "uuid-frank".into(),
                spine_index: -1,
                offset: None,
                limit: None,
            }))
            .await,
    );
    assert!(
        err.message.contains("non-negative"),
        "message: {}",
        err.message
    );
}

#[tokio::test]
async fn read_chapter_text_rejects_a_uuid_that_is_not_one_path_segment() {
    // A slash-bearing "uuid" would splice extra segments into the request
    // path; the shared guard answers invalid params before any request forms.
    let service = stub_service().await;
    let err = expect_err(
        service
            .read_chapter_text(Parameters(ChapterTextParams {
                book_uuid: "uuid-a/../uuid-b".into(),
                spine_index: 1,
                offset: None,
                limit: None,
            }))
            .await,
    );
    assert!(
        err.message.contains("book_uuid"),
        "message: {}",
        err.message
    );
}
