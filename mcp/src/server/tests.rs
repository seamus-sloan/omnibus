use std::sync::Arc;

use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use rmcp::handler::server::wrapper::Parameters;

use omnibus_shared::{EbookLibrary, EbookMetadata};

use super::*;
use crate::config::Config;
use crate::tools::read::{BookRef, ListBooksParams};

/// Every read tool this issue ships, by MCP tool name.
const EXPECTED_TOOLS: &[&str] = &[
    "library_overview",
    "list_books",
    "get_book",
    "search_books",
    "list_authors",
    "get_author",
    "list_series",
    "get_series",
    "list_tags",
    "list_genres",
    "list_shelves",
    "get_shelf",
    "shelves_containing_book",
    "reading_stats",
    "reading_sessions",
    "recent_progress",
    "book_progress",
    "book_read_status",
    "book_highlights",
    "book_bookmarks",
    "book_journal_entries",
];

fn offline_server() -> OmnibusMcp {
    let client = OmnibusClient::new(Config {
        base_url: "http://127.0.0.1:1".into(),
        username: "reader".into(),
        password: "correct horse battery".into(),
    })
    .unwrap();
    OmnibusMcp::new(Arc::new(client))
}

#[test]
fn read_tools_router_lists_every_expected_tool_with_a_description() {
    let router = OmnibusMcp::read_tools();
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

#[test]
fn get_info_declares_tools_and_readonly_instructions() {
    let info = offline_server().get_info();
    assert!(info.capabilities.tools.is_some());
    let instructions = info.instructions.unwrap_or_default();
    assert!(instructions.contains("Read-only"));
}

/// Boot a stub instance serving shared-typed JSON and return a service
/// pointed at it.
async fn stub_service() -> OmnibusMcp {
    async fn login() -> AxumJson<serde_json::Value> {
        AxumJson(serde_json::json!({
            "user": {
                "id": 1, "username": "reader", "is_admin": false,
                "can_upload": false, "can_edit": false, "can_download": true
            },
            "token": "token-1",
        }))
    }
    async fn book() -> AxumJson<EbookMetadata> {
        AxumJson(EbookMetadata {
            id: 7,
            filename: "frankenstein.epub".into(),
            title: Some("Frankenstein".into()),
            unique_identifier: Some("uuid-frank".into()),
            ..EbookMetadata::default()
        })
    }
    async fn ebooks() -> (HeaderMap, AxumJson<EbookLibrary>) {
        let mut headers = HeaderMap::new();
        headers.insert("x-total-count", "1".parse().unwrap());
        headers.insert("x-next-cursor", "cursor-2".parse().unwrap());
        let library = EbookLibrary {
            path: Some("/library".into()),
            books: vec![EbookMetadata {
                id: 7,
                filename: "frankenstein.epub".into(),
                ..EbookMetadata::default()
            }],
            error: None,
            total: None,
        };
        (headers, AxumJson(library))
    }
    let app = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/ebooks", get(ebooks))
        .route("/api/ebooks/uuid-frank", get(book));
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

#[tokio::test]
async fn get_book_tool_returns_the_shared_typed_book() {
    let service = stub_service().await;
    let book = service
        .get_book(Parameters(BookRef {
            uuid: "uuid-frank".into(),
        }))
        .await
        .unwrap();
    assert_eq!(book.0.title.as_deref(), Some("Frankenstein"));
    assert_eq!(book.0.unique_identifier.as_deref(), Some("uuid-frank"));
}

#[tokio::test]
async fn get_book_tool_reports_not_found_for_an_unknown_uuid() {
    let service = stub_service().await;
    let err = match service
        .get_book(Parameters(BookRef {
            uuid: "uuid-missing".into(),
        }))
        .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected a not-found error"),
    };
    assert!(err.message.contains("not found"));
}

#[tokio::test]
async fn list_books_tool_carries_the_header_pagination_metadata() {
    let service = stub_service().await;
    let page = service
        .list_books(Parameters(ListBooksParams::default()))
        .await
        .unwrap();
    assert_eq!(page.0.library.books.len(), 1);
    assert_eq!(page.0.next_cursor.as_deref(), Some("cursor-2"));
    assert_eq!(page.0.total, Some(1));
}
