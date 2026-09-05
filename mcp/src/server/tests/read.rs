//! The read-only family: every expected tool is listed with a
//! description, `get_info` declares the confirm-gated write surface and
//! the icon, and `get_book` / `list_books` return the shared typed
//! payloads with their not-found and pagination metadata.

use std::sync::Arc;

use axum::http::HeaderMap;
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use rmcp::handler::server::wrapper::Parameters;

use omnibus_shared::{EbookLibrary, EbookMetadata};

use super::super::*;
use super::offline_server;
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
fn get_info_declares_tools_and_the_confirm_gated_write_surface() {
    let info = offline_server().get_info();
    assert!(info.capabilities.tools.is_some());
    let instructions = info.instructions.unwrap_or_default();
    assert!(instructions.contains("Read-only"));
    assert!(instructions.contains("confirm=true"));
    assert!(instructions.contains("preview_shelf_rule"));
    assert!(instructions.contains("confirm: true"));
    assert!(instructions.contains("propose_metadata_changes"));
    assert!(instructions.contains("merge_books"));
    assert!(instructions.contains("search_book_content"));
}

#[test]
fn get_info_advertises_the_stoat_icon_as_a_png_data_uri() {
    let info = offline_server().get_info();
    let icons = info.server_info.icons.expect("serverInfo.icons declared");
    assert_eq!(icons.len(), 1);
    assert!(icons[0].src.starts_with("data:image/png;base64,"));
    assert_eq!(icons[0].mime_type.as_deref(), Some("image/png"));
    assert_eq!(
        icons[0].sizes.as_deref(),
        Some(&["128x128".to_string()][..])
    );
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
