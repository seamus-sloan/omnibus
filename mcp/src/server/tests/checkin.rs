//! The physical check-in family against a recording stub: tool listing
//! and the combined router, ISBN / title lookups naming their provider,
//! the confirm gates on check-in and copy removal, the wishlist add and
//! remove paths, copy listing and note edits, the 403 permission names,
//! and the one-path-segment uuid guard.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::sync::Mutex;

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{delete, patch};
use axum::routing::{get, post};
use axum::{Json as AxumJson, Router};
use omnibus_shared::{
    ExternalBookMeta, MetadataProvider, PhysicalCopy, ScanBook, ScanOutcome, ScanSearchResponse,
};
use rmcp::handler::server::wrapper::Parameters;

use super::super::*;
use super::{offline_server, ExpectErrData};
use crate::config::Config;
use crate::tools::checkin::{
    AddToWishlistParams, BookUuid, CheckInParams, LookupIsbnParams, RemoveCopyParams,
    ResolveMetadataParams, SearchMetadataParams, UpdateCopyNoteParams,
};

/// Every check-in-family tool this issue ships, by MCP tool name.
const EXPECTED_CHECKIN_TOOLS: &[&str] = &[
    "lookup_isbn",
    "search_book_metadata",
    "resolve_book_metadata",
    "check_in_physical_book",
    "add_to_wishlist",
    "remove_from_wishlist",
    "list_physical_copies",
    "update_copy_note",
    "remove_physical_copy",
];

/// Stub write-route state: records mutation bodies and counts destructive
/// calls so tests can assert what did (or did not) reach the server.
#[derive(Default)]
struct WriteStub {
    check_ins: AtomicUsize,
    last_check_in: Mutex<Option<serde_json::Value>>,
    last_wishlist_add: Mutex<Option<serde_json::Value>>,
    wishlist_deletes: Mutex<Vec<String>>,
    copy_deletes: AtomicUsize,
}

fn provider_meta(source: MetadataProvider) -> ExternalBookMeta {
    ExternalBookMeta {
        isbn13: "9780306406157".into(),
        title: "Voyage of the Beagle".into(),
        authors: vec!["Charles Darwin".into()],
        year: None,
        pages: None,
        publisher: None,
        description: None,
        cover_url: None,
        series: None,
        first_publish_year: None,
        source,
    }
}

fn library_book() -> ScanBook {
    ScanBook {
        uuid: "uuid-frank".into(),
        title: "Frankenstein".into(),
        authors: vec!["Mary Shelley".into()],
        cover_url: None,
        has_physical: false,
        isbn: None,
    }
}

async fn resolve(AxumJson(body): AxumJson<serde_json::Value>) -> axum::response::Response {
    use axum::response::IntoResponse;
    match body["isbn"].as_str() {
        Some("9780306406157") => AxumJson(ScanOutcome::NotInLibrary {
            online: provider_meta(MetadataProvider::GoogleBooks),
        })
        .into_response(),
        Some("9781111111111") => AxumJson(ScanOutcome::InLibraryUnowned {
            book: library_book(),
        })
        .into_response(),
        Some("bad") => (StatusCode::BAD_REQUEST, "invalid ISBN checksum").into_response(),
        _ => AxumJson(ScanOutcome::Unresolved).into_response(),
    }
}

async fn resolve_meta() -> AxumJson<ScanOutcome> {
    AxumJson(ScanOutcome::CloseMatch {
        book: library_book(),
        others: vec![],
        scanned: provider_meta(MetadataProvider::OpenLibrary),
    })
}

async fn search() -> AxumJson<ScanSearchResponse> {
    AxumJson(ScanSearchResponse {
        results: vec![provider_meta(MetadataProvider::OpenLibrary)],
    })
}

async fn check_in(
    State(stub): State<Arc<WriteStub>>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> AxumJson<serde_json::Value> {
    stub.check_ins.fetch_add(1, Ordering::SeqCst);
    *stub.last_check_in.lock().unwrap() = Some(body);
    AxumJson(serde_json::json!({ "book_uuid": "uuid-frank" }))
}

async fn wishlist_add(
    State(stub): State<Arc<WriteStub>>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> AxumJson<serde_json::Value> {
    *stub.last_wishlist_add.lock().unwrap() = Some(body);
    AxumJson(serde_json::json!({ "book_uuid": "uuid-frank" }))
}

async fn wishlist_delete(
    State(stub): State<Arc<WriteStub>>,
    Path(uuid): Path<String>,
) -> StatusCode {
    stub.wishlist_deletes.lock().unwrap().push(uuid);
    StatusCode::NO_CONTENT
}

fn copy(note: Option<String>) -> PhysicalCopy {
    PhysicalCopy {
        id: 5,
        book_uuid: "uuid-frank".into(),
        isbn: Some("9781111111111".into()),
        added_by_user_id: Some(1),
        checked_in_at: 1_700_000_000,
        note,
    }
}

async fn copies() -> AxumJson<Vec<PhysicalCopy>> {
    AxumJson(vec![copy(Some("hardcover".into()))])
}

/// Copy 5 accepts edits; copy 6 answers the `can_edit` 403 the real handler
/// sends; anything else is a 404.
async fn patch_copy(
    Path(copy_id): Path<i64>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match copy_id {
        5 => AxumJson(copy(body["note"].as_str().map(str::to_string))).into_response(),
        6 => (StatusCode::FORBIDDEN, "edit permission required").into_response(),
        _ => (StatusCode::NOT_FOUND, "physical copy not found").into_response(),
    }
}

async fn delete_copy(
    State(stub): State<Arc<WriteStub>>,
    Path(copy_id): Path<i64>,
) -> axum::response::Response {
    use axum::response::IntoResponse;
    match copy_id {
        5 => {
            stub.copy_deletes.fetch_add(1, Ordering::SeqCst);
            StatusCode::NO_CONTENT.into_response()
        }
        6 => (StatusCode::FORBIDDEN, "edit permission required").into_response(),
        _ => (StatusCode::NOT_FOUND, "physical copy not found").into_response(),
    }
}

/// Boot a stub instance carrying the scan/physical routes and return the
/// service plus the recorded-writes handle.
async fn checkin_stub_service() -> (OmnibusMcp, Arc<WriteStub>) {
    async fn login() -> AxumJson<serde_json::Value> {
        AxumJson(serde_json::json!({
            "user": {
                "id": 1, "username": "reader", "is_admin": false,
                "can_upload": false, "can_edit": true, "can_download": true
            },
            "token": "token-1",
        }))
    }
    let stub = Arc::new(WriteStub::default());
    let app = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/scan/resolve", post(resolve))
        .route("/api/scan/search", post(search))
        .route("/api/scan/resolve-meta", post(resolve_meta))
        .route("/api/scan/check-in", post(check_in))
        .route("/api/scan/wishlist", post(wishlist_add))
        .route("/api/physical/{uuid}/wishlist", delete(wishlist_delete))
        .route("/api/physical/{uuid}/copies", get(copies))
        .route(
            "/api/physical/copies/{copy_id}",
            patch(patch_copy).delete(delete_copy),
        )
        .with_state(stub.clone());
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
    (OmnibusMcp::new(Arc::new(client)), stub)
}

#[test]
fn checkin_tools_router_lists_every_expected_tool_with_a_description() {
    let router = OmnibusMcp::checkin_tools();
    let tools = router.list_all();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in EXPECTED_CHECKIN_TOOLS {
        assert!(names.contains(expected), "missing tool {expected}");
    }
    assert_eq!(
        tools.len(),
        EXPECTED_CHECKIN_TOOLS.len(),
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
fn combined_router_carries_every_family_without_collisions() {
    // Derived from the per-family routers, not hardcoded: a name collision
    // between families would make the combined router smaller than the sum.
    let per_family: usize = [
        OmnibusMcp::read_tools().list_all().len(),
        OmnibusMcp::checkin_tools().list_all().len(),
        OmnibusMcp::shelf_tools().list_all().len(),
        OmnibusMcp::metadata_tools().list_all().len(),
        OmnibusMcp::merge_tools().list_all().len(),
        OmnibusMcp::content_tools().list_all().len(),
    ]
    .iter()
    .sum();
    let combined = offline_server().tool_router;
    assert_eq!(combined.list_all().len(), per_family);
    // 21 read + 9 checkin + 6 shelf + 6 metadata + 2 merge + 3 content.
    assert_eq!(per_family, 47);
}

#[tokio::test]
async fn lookup_isbn_names_the_provider_and_explains_every_no_match() {
    let (service, _stub) = checkin_stub_service().await;
    let results = service
        .lookup_isbn(Parameters(LookupIsbnParams {
            isbns: vec![
                "9780306406157".into(),
                "9781111111111".into(),
                "9999999999999".into(),
            ],
        }))
        .await
        .unwrap();
    let [provider_hit, library_hit, unresolved] = results.0.as_slice() else {
        panic!("expected three rows, got {}", results.0.len());
    };

    // Provider-resolved: the answering provider is named.
    assert_eq!(provider_hit.provider.as_deref(), Some("Google Books"));
    assert!(matches!(
        provider_hit.outcome,
        Some(ScanOutcome::NotInLibrary { .. })
    ));

    // Library-exact: no provider involved, and the detail says so.
    assert_eq!(library_hit.provider, None);
    assert!(matches!(
        library_hit.outcome,
        Some(ScanOutcome::InLibraryUnowned { .. })
    ));
    assert!(library_hit.detail.as_deref().unwrap().contains("library"));

    // Unresolved: reported with an explanation, never dropped.
    assert!(matches!(unresolved.outcome, Some(ScanOutcome::Unresolved)));
    assert!(unresolved.detail.as_deref().unwrap().contains("provider"));
}

#[tokio::test]
async fn lookup_isbn_reports_an_invalid_isbn_as_a_structured_row() {
    let (service, _stub) = checkin_stub_service().await;
    let results = service
        .lookup_isbn(Parameters(LookupIsbnParams {
            isbns: vec!["bad".into(), "9781111111111".into()],
        }))
        .await
        .unwrap();
    assert!(results.0[0].outcome.is_none());
    let detail = results.0[0].detail.as_deref().unwrap();
    assert!(detail.contains("HTTP 400") && detail.contains("invalid ISBN checksum"));
    // The failure did not swallow the rest of the batch.
    assert!(results.0[1].outcome.is_some());
}

#[tokio::test]
async fn search_book_metadata_returns_provider_attributed_candidates() {
    let (service, _stub) = checkin_stub_service().await;
    let response = service
        .search_book_metadata(Parameters(SearchMetadataParams {
            query: "voyage of the beagle".into(),
        }))
        .await
        .unwrap();
    assert_eq!(response.0.results.len(), 1);
    assert_eq!(response.0.results[0].source, MetadataProvider::OpenLibrary);
}

#[tokio::test]
async fn resolve_book_metadata_reports_the_close_match_with_its_provider() {
    let (service, _stub) = checkin_stub_service().await;
    let resolution = service
        .resolve_book_metadata(Parameters(ResolveMetadataParams {
            meta: provider_meta(MetadataProvider::OpenLibrary),
        }))
        .await
        .unwrap();
    assert!(matches!(
        resolution.0.outcome,
        Some(ScanOutcome::CloseMatch { .. })
    ));
    assert_eq!(resolution.0.provider.as_deref(), Some("Open Library"));
}

#[tokio::test]
async fn check_in_physical_book_refuses_without_confirm_and_sends_nothing() {
    let (service, stub) = checkin_stub_service().await;
    let err = service
        .check_in_physical_book(Parameters(CheckInParams {
            book_uuid: "uuid-frank".into(),
            isbn: Some("9781111111111".into()),
            note: None,
            confirm: false,
        }))
        .await
        .expect_err_data();
    assert!(err.message.contains("confirm"));
    assert_eq!(stub.check_ins.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn check_in_physical_book_sends_the_expected_body_when_confirmed() {
    let (service, stub) = checkin_stub_service().await;
    let receipt = service
        .check_in_physical_book(Parameters(CheckInParams {
            book_uuid: "uuid-frank".into(),
            isbn: Some("9781111111111".into()),
            note: Some("shop find".into()),
            confirm: true,
        }))
        .await
        .unwrap();
    assert_eq!(receipt.0.book_uuid, "uuid-frank");
    let body = stub.last_check_in.lock().unwrap().clone().unwrap();
    assert_eq!(body["book_uuid"], "uuid-frank");
    assert_eq!(body["isbn"], "9781111111111");
    assert_eq!(body["note"], "shop find");
}

#[tokio::test]
async fn add_to_wishlist_posts_the_library_uuid_with_detail_source() {
    let (service, stub) = checkin_stub_service().await;
    let landed = service
        .add_to_wishlist(Parameters(AddToWishlistParams {
            uuid: Some("uuid-frank".into()),
            meta: None,
        }))
        .await
        .unwrap();
    assert_eq!(landed.0.book_uuid, "uuid-frank");
    let body = stub.last_wishlist_add.lock().unwrap().clone().unwrap();
    assert_eq!(body["book_uuid"], "uuid-frank");
    assert_eq!(body["source"], "detail");
}

#[tokio::test]
async fn add_to_wishlist_posts_external_meta_with_search_source() {
    let (service, stub) = checkin_stub_service().await;
    service
        .add_to_wishlist(Parameters(AddToWishlistParams {
            uuid: None,
            meta: Some(provider_meta(MetadataProvider::GoogleBooks)),
        }))
        .await
        .unwrap();
    let body = stub.last_wishlist_add.lock().unwrap().clone().unwrap();
    assert_eq!(body["meta"]["isbn13"], "9780306406157");
    assert_eq!(body["source"], "search");
}

#[tokio::test]
async fn add_to_wishlist_refuses_when_neither_uuid_nor_meta_is_given() {
    let (service, stub) = checkin_stub_service().await;
    let err = service
        .add_to_wishlist(Parameters(AddToWishlistParams {
            uuid: None,
            meta: None,
        }))
        .await
        .expect_err_data();
    assert!(err.message.contains("uuid") && err.message.contains("meta"));
    assert!(stub.last_wishlist_add.lock().unwrap().is_none());
}

#[tokio::test]
async fn remove_from_wishlist_deletes_the_entry() {
    let (service, stub) = checkin_stub_service().await;
    let ack = service
        .remove_from_wishlist(Parameters(BookUuid {
            uuid: "uuid-frank".into(),
        }))
        .await
        .unwrap();
    assert!(ack.0.message.contains("uuid-frank"));
    assert_eq!(
        stub.wishlist_deletes.lock().unwrap().clone(),
        vec!["uuid-frank".to_string()]
    );
}

#[tokio::test]
async fn list_physical_copies_returns_the_shared_typed_copies() {
    let (service, _stub) = checkin_stub_service().await;
    let copies = service
        .list_physical_copies(Parameters(BookUuid {
            uuid: "uuid-frank".into(),
        }))
        .await
        .unwrap();
    assert_eq!(copies.0.len(), 1);
    assert_eq!(copies.0[0].id, 5);
    assert_eq!(copies.0[0].note.as_deref(), Some("hardcover"));
}

#[tokio::test]
async fn update_copy_note_patches_and_returns_the_copy() {
    let (service, _stub) = checkin_stub_service().await;
    let updated = service
        .update_copy_note(Parameters(UpdateCopyNoteParams {
            copy_id: 5,
            note: Some("signed first edition".into()),
        }))
        .await
        .unwrap();
    assert_eq!(updated.0.note.as_deref(), Some("signed first edition"));
}

#[tokio::test]
async fn update_copy_note_names_the_missing_edit_permission_on_403() {
    let (service, _stub) = checkin_stub_service().await;
    let err = service
        .update_copy_note(Parameters(UpdateCopyNoteParams {
            copy_id: 6,
            note: Some("nope".into()),
        }))
        .await
        .expect_err_data();
    assert!(err.message.contains("can_edit"), "got: {}", err.message);
    assert!(err.message.contains("edit permission required"));
}

#[tokio::test]
async fn remove_physical_copy_refuses_without_confirm() {
    let (service, stub) = checkin_stub_service().await;
    let err = service
        .remove_physical_copy(Parameters(RemoveCopyParams {
            copy_id: 5,
            confirm: false,
        }))
        .await
        .expect_err_data();
    assert!(err.message.contains("confirm"));
    assert_eq!(stub.copy_deletes.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn remove_physical_copy_deletes_when_confirmed() {
    let (service, stub) = checkin_stub_service().await;
    let ack = service
        .remove_physical_copy(Parameters(RemoveCopyParams {
            copy_id: 5,
            confirm: true,
        }))
        .await
        .unwrap();
    assert!(ack.0.message.contains('5'));
    assert_eq!(stub.copy_deletes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn remove_physical_copy_names_the_missing_edit_permission_on_403() {
    let (service, _stub) = checkin_stub_service().await;
    let err = service
        .remove_physical_copy(Parameters(RemoveCopyParams {
            copy_id: 6,
            confirm: true,
        }))
        .await
        .expect_err_data();
    assert!(err.message.contains("can_edit"), "got: {}", err.message);
}

#[tokio::test]
async fn remove_from_wishlist_rejects_a_uuid_that_is_not_one_path_segment() {
    let service = offline_server();
    let err = match service
        .remove_from_wishlist(Parameters(crate::tools::checkin::BookUuid {
            uuid: "a/b".into(),
        }))
        .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected an invalid-params error"),
    };
    assert!(err.message.contains("uuid"), "message: {}", err.message);
}
