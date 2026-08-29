use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use rmcp::handler::server::wrapper::Parameters;

use omnibus_shared::metadata_lookup::{
    EditionSearchRequest, EditionSearchResponse, MetadataProvider, ProviderEdition,
    ProviderSearchSource, ProviderSearchStatus,
};
use omnibus_shared::{EbookMetadata, MetadataOverrides};

use super::*;
use crate::client::OmnibusClient;
use crate::config::Config;

/// Every metadata tool this issue ships, by MCP tool name.
const EXPECTED_TOOLS: &[&str] = &[
    "get_effective_metadata",
    "propose_metadata_changes",
    "apply_metadata_changes",
    "revert_metadata_overrides",
    "search_metadata_providers",
    "hydrate_provider_edition",
];

const KNOWN: &[&str] = &["uuid-1", "uuid-2", "uuid-3"];

/// Stub instance state: records override writes and lets a test force a
/// failure status for one uuid's mutating requests.
#[derive(Default)]
struct Stub {
    posts: Mutex<Vec<(String, serde_json::Value)>>,
    deletes: Mutex<Vec<String>>,
    fail: Mutex<HashMap<String, u16>>,
}

fn stub_book(uuid: &str) -> EbookMetadata {
    EbookMetadata {
        id: 7,
        filename: format!("{uuid}.epub"),
        title: Some("Old Title".into()),
        series: Some("Old Series".into()),
        subjects: vec!["scanned-tag".into()],
        unique_identifier: Some(uuid.into()),
        ..EbookMetadata::default()
    }
}

async fn login() -> AxumJson<serde_json::Value> {
    AxumJson(serde_json::json!({
        "user": {
            "id": 1, "username": "editor", "is_admin": false,
            "can_upload": false, "can_edit": true, "can_download": true
        },
        "token": "token-1",
    }))
}

async fn get_book(Path(uuid): Path<String>) -> Result<AxumJson<EbookMetadata>, StatusCode> {
    if KNOWN.contains(&uuid.as_str()) {
        Ok(AxumJson(stub_book(&uuid)))
    } else {
        Err(StatusCode::NOT_FOUND)
    }
}

async fn post_overrides(
    State(stub): State<Arc<Stub>>,
    Path(uuid): Path<String>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> Result<AxumJson<EbookMetadata>, StatusCode> {
    if let Some(status) = stub.fail.lock().unwrap().get(&uuid) {
        return Err(StatusCode::from_u16(*status).unwrap());
    }
    stub.posts.lock().unwrap().push((uuid.clone(), body));
    let mut book = stub_book(&uuid);
    book.has_override = true;
    Ok(AxumJson(book))
}

async fn delete_overrides(
    State(stub): State<Arc<Stub>>,
    Path(uuid): Path<String>,
) -> Result<AxumJson<EbookMetadata>, StatusCode> {
    if let Some(status) = stub.fail.lock().unwrap().get(&uuid) {
        return Err(StatusCode::from_u16(*status).unwrap());
    }
    stub.deletes.lock().unwrap().push(uuid.clone());
    Ok(AxumJson(stub_book(&uuid)))
}

async fn edition_search(
    AxumJson(req): AxumJson<EditionSearchRequest>,
) -> AxumJson<EditionSearchResponse> {
    AxumJson(EditionSearchResponse {
        editions: vec![ProviderEdition {
            source: MetadataProvider::OpenLibrary,
            provider_ref: "OL123W".into(),
            isbn13: None,
            isbn10: None,
            title: req.query,
            authors: vec!["Mary Shelley".into()],
            year: Some("1818".into()),
            pages: None,
            publisher: None,
            description: None,
            cover_url: None,
            series: None,
            series_index: None,
            first_publish_year: Some(1818),
            genres: vec![],
            relevance: Some(1000),
        }],
        sources: vec![ProviderSearchSource {
            provider: MetadataProvider::OpenLibrary,
            display_name: "Open Library".into(),
            status: ProviderSearchStatus::Answered { count: 1 },
        }],
    })
}

/// Boot a stub instance and return a service pointed at it plus the stub.
async fn stub_service() -> (OmnibusMcp, Arc<Stub>) {
    let stub = Arc::new(Stub::default());
    let app = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/ebooks/{uuid}", get(get_book))
        .route(
            "/api/ebooks/{uuid}/overrides",
            post(post_overrides).delete(delete_overrides),
        )
        .route("/api/metadata/editions/search", post(edition_search))
        .with_state(stub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = OmnibusClient::new(Config {
        base_url: format!("http://{addr}"),
        username: "editor".into(),
        password: "correct horse battery".into(),
    })
    .unwrap();
    (OmnibusMcp::new(Arc::new(client)), stub)
}

/// `unwrap_err` needs `T: Debug`, which `rmcp::Json` does not implement.
fn expect_err<T>(result: Result<T, ErrorData>) -> ErrorData {
    match result {
        Err(e) => e,
        Ok(_) => panic!("expected an error"),
    }
}

fn title_change(uuid: &str, title: &str) -> BookChange {
    BookChange {
        book_uuid: uuid.into(),
        changes: MetadataOverrides {
            title: Some(title.into()),
            ..MetadataOverrides::default()
        },
    }
}

#[test]
fn metadata_tools_router_lists_every_expected_tool_with_a_description() {
    let router = OmnibusMcp::metadata_tools();
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
async fn get_effective_metadata_returns_each_requested_book_in_order() {
    let (service, _stub) = stub_service().await;
    let books = service
        .get_effective_metadata(Parameters(BookSetParams {
            uuids: vec!["uuid-1".into(), "uuid-2".into()],
        }))
        .await
        .unwrap();
    assert_eq!(books.0.len(), 2);
    assert_eq!(books.0[0].unique_identifier.as_deref(), Some("uuid-1"));
    assert_eq!(books.0[1].unique_identifier.as_deref(), Some("uuid-2"));
}

#[tokio::test]
async fn get_effective_metadata_reports_not_found_for_an_unknown_uuid() {
    let (service, _stub) = stub_service().await;
    let err = expect_err(
        service
            .get_effective_metadata(Parameters(BookSetParams {
                uuids: vec!["uuid-missing".into()],
            }))
            .await,
    );
    assert!(err.message.contains("uuid-missing"));
    assert!(err.message.contains("not found"));
}

#[tokio::test]
async fn propose_metadata_changes_diffs_current_values_and_writes_nothing() {
    let (service, stub) = stub_service().await;
    let proposed = service
        .propose_metadata_changes(Parameters(ProposeParams {
            changes: vec![BookChange {
                book_uuid: "uuid-1".into(),
                changes: MetadataOverrides {
                    title: Some("New Title".into()),
                    series: Some("New Series".into()),
                    ..MetadataOverrides::default()
                },
            }],
        }))
        .await
        .unwrap();

    let book = &proposed.0.books[0];
    assert_eq!(book.book_uuid, "uuid-1");
    assert!(!book.already_has_override);
    assert_eq!(book.fields.len(), 2);
    let title = book.fields.iter().find(|f| f.field == "title").unwrap();
    assert_eq!(title.before, serde_json::json!("Old Title"));
    assert_eq!(title.after, serde_json::json!("New Title"));
    assert!(title.note.is_none());
    let series = book.fields.iter().find(|f| f.field == "series").unwrap();
    assert_eq!(series.before, serde_json::json!("Old Series"));
    assert_eq!(series.after, serde_json::json!("New Series"));
    assert!(proposed.0.next_step.contains("confirm: true"));

    // A dry run: nothing reached the override endpoints.
    assert!(stub.posts.lock().unwrap().is_empty());
    assert!(stub.deletes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn propose_metadata_changes_annotates_a_genre_change_as_establishing_an_override() {
    let (service, _stub) = stub_service().await;
    let proposed = service
        .propose_metadata_changes(Parameters(ProposeParams {
            changes: vec![BookChange {
                book_uuid: "uuid-1".into(),
                changes: MetadataOverrides {
                    genres: Some(vec!["Gothic".into(), "Horror".into()]),
                    ..MetadataOverrides::default()
                },
            }],
        }))
        .await
        .unwrap();
    let genres = &proposed.0.books[0].fields[0];
    assert_eq!(genres.field, "genres");
    assert_eq!(genres.before, serde_json::json!([]));
    assert_eq!(genres.after, serde_json::json!(["Gothic", "Horror"]));
    let note = genres.note.as_deref().unwrap();
    assert!(note.contains("override-only"));
    assert!(note.contains("establishes a metadata override"));
}

#[tokio::test]
async fn propose_metadata_changes_rejects_an_entry_that_names_no_fields() {
    let (service, _stub) = stub_service().await;
    let err = expect_err(
        service
            .propose_metadata_changes(Parameters(ProposeParams {
                changes: vec![BookChange {
                    book_uuid: "uuid-1".into(),
                    changes: MetadataOverrides::default(),
                }],
            }))
            .await,
    );
    assert!(err.message.contains("names no fields"));
}

#[tokio::test]
async fn apply_metadata_changes_refuses_without_confirm() {
    let (service, stub) = stub_service().await;
    for confirm in [None, Some(false)] {
        let err = expect_err(
            service
                .apply_metadata_changes(Parameters(ApplyParams {
                    changes: vec![title_change("uuid-1", "New Title")],
                    confirm,
                }))
                .await,
        );
        assert!(err.message.contains("refused"));
        assert!(err.message.contains("confirm: true"));
        assert!(err.message.contains("propose_metadata_changes"));
    }
    assert!(stub.posts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn apply_metadata_changes_posts_each_books_overrides_with_the_expected_body() {
    let (service, stub) = stub_service().await;
    let report = service
        .apply_metadata_changes(Parameters(ApplyParams {
            changes: vec![
                title_change("uuid-1", "New Title"),
                title_change("uuid-2", "Other Title"),
            ],
            confirm: Some(true),
        }))
        .await
        .unwrap();

    let posts = stub.posts.lock().unwrap().clone();
    assert_eq!(
        posts,
        vec![
            (
                "uuid-1".to_string(),
                serde_json::json!({"title": "New Title"})
            ),
            (
                "uuid-2".to_string(),
                serde_json::json!({"title": "Other Title"})
            ),
        ]
    );
    assert_eq!(report.0.applied.len(), 2);
    assert_eq!(report.0.applied[0].book_uuid, "uuid-1");
    assert!(report.0.applied[0].has_override);
}

#[tokio::test]
async fn apply_metadata_changes_names_the_missing_edit_permission_on_a_403() {
    let (service, stub) = stub_service().await;
    stub.fail.lock().unwrap().insert("uuid-1".into(), 403);
    let err = expect_err(
        service
            .apply_metadata_changes(Parameters(ApplyParams {
                changes: vec![title_change("uuid-1", "New Title")],
                confirm: Some(true),
            }))
            .await,
    );
    assert!(err.message.contains("can_edit"), "got: {}", err.message);
    assert!(err.message.contains("edit permission"));
    assert!(stub.posts.lock().unwrap().is_empty());
}

#[tokio::test]
async fn apply_metadata_changes_reports_partial_application_on_a_mid_batch_failure() {
    let (service, stub) = stub_service().await;
    stub.fail.lock().unwrap().insert("uuid-2".into(), 500);
    let err = expect_err(
        service
            .apply_metadata_changes(Parameters(ApplyParams {
                changes: vec![
                    title_change("uuid-1", "A"),
                    title_change("uuid-2", "B"),
                    title_change("uuid-3", "C"),
                ],
                confirm: Some(true),
            }))
            .await,
    );

    // Only the first write landed; the failure stopped the batch.
    let posts = stub.posts.lock().unwrap().clone();
    assert_eq!(posts.len(), 1);
    assert_eq!(posts[0].0, "uuid-1");

    // The report names all three dispositions.
    assert!(err.message.contains("uuid-1"));
    assert!(err.message.contains("uuid-2"));
    assert!(err.message.contains("uuid-3"));
    let data = err.data.unwrap();
    assert_eq!(data["written"], serde_json::json!(["uuid-1"]));
    assert_eq!(data["failed"]["book_uuid"], serde_json::json!("uuid-2"));
    assert_eq!(data["not_attempted"], serde_json::json!(["uuid-3"]));
}

#[tokio::test]
async fn revert_metadata_overrides_refuses_without_confirm() {
    let (service, stub) = stub_service().await;
    let err = expect_err(
        service
            .revert_metadata_overrides(Parameters(RevertParams {
                uuids: vec!["uuid-1".into()],
                confirm: None,
            }))
            .await,
    );
    assert!(err.message.contains("refused"));
    assert!(err.message.contains("confirm: true"));
    assert!(stub.deletes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn revert_metadata_overrides_deletes_each_books_overrides() {
    let (service, stub) = stub_service().await;
    let report = service
        .revert_metadata_overrides(Parameters(RevertParams {
            uuids: vec!["uuid-1".into(), "uuid-2".into()],
            confirm: Some(true),
        }))
        .await
        .unwrap();
    assert_eq!(
        stub.deletes.lock().unwrap().clone(),
        vec!["uuid-1".to_string(), "uuid-2".to_string()]
    );
    assert_eq!(report.0.reverted.len(), 2);
    assert!(!report.0.reverted[0].has_override);
}

#[tokio::test]
async fn search_metadata_providers_returns_the_attributed_fan_out() {
    let (service, _stub) = stub_service().await;
    let found = service
        .search_metadata_providers(Parameters(EditionSearchRequest {
            query: "Frankenstein".into(),
            ..EditionSearchRequest::default()
        }))
        .await
        .unwrap();
    assert_eq!(found.0.editions.len(), 1);
    assert_eq!(found.0.editions[0].title, "Frankenstein");
    assert_eq!(found.0.editions[0].provider_ref, "OL123W");
    assert_eq!(
        found.0.sources[0].status,
        ProviderSearchStatus::Answered { count: 1 }
    );
}

#[tokio::test]
async fn apply_metadata_changes_rejects_a_uuid_that_is_not_one_path_segment() {
    // A slash-bearing "uuid" would split the overrides path into extra
    // segments and trip the WRITE_ALLOWLIST assert — the up-front guard must
    // answer invalid params before the first write.
    let (service, stub) = stub_service().await;
    let err = expect_err(
        service
            .apply_metadata_changes(Parameters(ApplyParams {
                changes: vec![title_change("uuid-1/../uuid-2", "Frankenstein")],
                confirm: Some(true),
            }))
            .await,
    );
    assert!(
        err.message.contains("book_uuid"),
        "message: {}",
        err.message
    );
    assert!(stub.posts.lock().unwrap().is_empty());
}
