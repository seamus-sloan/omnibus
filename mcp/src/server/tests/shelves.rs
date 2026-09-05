//! The shelf family against a recording stub: tool listing, preview
//! creating nothing and create sending the previewed rule, membership
//! add / remove, the confirm gate on delete, the ownership 403, the
//! one-path-segment uuid guard, and the local kind-combination checks.

use std::sync::Arc;

use axum::routing::post;
use axum::{Json as AxumJson, Router};
use rmcp::handler::server::wrapper::Parameters;

use omnibus_shared::{
    EbookMetadata, MatchMode, RuleField, RuleOp, RulePreview, Shelf, ShelfKind, ShelfRule,
    Visibility,
};

use super::super::*;
use super::offline_server;
use crate::config::Config;
use crate::tools::shelves::{
    AddBooksParams, CreateShelfParams, DeleteShelfParams, PreviewRuleParams, RemoveBookParams,
    UpdateShelfParams,
};

/// Every shelf-authoring tool, by MCP tool name.
const EXPECTED_SHELF_TOOLS: &[&str] = &[
    "preview_shelf_rule",
    "create_shelf",
    "update_shelf",
    "add_books_to_shelf",
    "remove_book_from_shelf",
    "delete_shelf",
];

#[test]
fn shelf_tools_router_lists_every_expected_tool_with_a_description() {
    let router = OmnibusMcp::shelf_tools();
    let tools = router.list_all();
    let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
    for expected in EXPECTED_SHELF_TOOLS {
        assert!(names.contains(expected), "missing tool {expected}");
    }
    assert_eq!(
        tools.len(),
        EXPECTED_SHELF_TOOLS.len(),
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

/// State recorded by the shelf stub: the last body per capturing route, plus
/// every bodyless-delete path that was hit.
#[derive(Default)]
struct ShelfStub {
    last_preview_body: std::sync::Mutex<Option<serde_json::Value>>,
    last_create_body: std::sync::Mutex<Option<serde_json::Value>>,
    last_add_body: std::sync::Mutex<Option<serde_json::Value>>,
    deletes: std::sync::Mutex<Vec<String>>,
}

fn sample_shelf() -> Shelf {
    Shelf {
        id: 5,
        owner_user_id: 1,
        owner_username: "reader".into(),
        kind: ShelfKind::Smart,
        name: "Le Guin EPUBs".into(),
        description: None,
        visibility: Visibility::Private,
        accent: None,
        match_mode: Some(MatchMode::All),
        rules: vec![ShelfRule {
            field: RuleField::Author,
            op: RuleOp::Contains,
            value: "le guin".into(),
        }],
        book_count: 2,
        sync_to_kobo: false,
    }
}

/// Boot a stub instance serving the shelf endpoints and return a service
/// pointed at it plus the recorded state.
async fn shelf_stub_service() -> (OmnibusMcp, Arc<ShelfStub>) {
    use axum::extract::State;
    use axum::http::StatusCode;
    use axum::routing::{delete, patch};

    async fn login() -> AxumJson<serde_json::Value> {
        AxumJson(serde_json::json!({
            "user": {
                "id": 1, "username": "reader", "is_admin": false,
                "can_upload": false, "can_edit": false, "can_download": true
            },
            "token": "token-1",
        }))
    }
    async fn preview(
        State(stub): State<Arc<ShelfStub>>,
        AxumJson(body): AxumJson<serde_json::Value>,
    ) -> AxumJson<RulePreview> {
        *stub.last_preview_body.lock().unwrap() = Some(body);
        AxumJson(RulePreview {
            matched: 2,
            total: 10,
            sample: vec![EbookMetadata {
                id: 7,
                filename: "dispossessed.epub".into(),
                title: Some("The Dispossessed".into()),
                unique_identifier: Some("uuid-dispossessed".into()),
                ..EbookMetadata::default()
            }],
        })
    }
    async fn create(
        State(stub): State<Arc<ShelfStub>>,
        AxumJson(body): AxumJson<serde_json::Value>,
    ) -> (StatusCode, AxumJson<Shelf>) {
        *stub.last_create_body.lock().unwrap() = Some(body);
        (StatusCode::CREATED, AxumJson(sample_shelf()))
    }
    async fn update_forbidden() -> (StatusCode, &'static str) {
        (StatusCode::FORBIDDEN, "not your shelf")
    }
    async fn add_books(
        State(stub): State<Arc<ShelfStub>>,
        AxumJson(body): AxumJson<serde_json::Value>,
    ) -> StatusCode {
        *stub.last_add_body.lock().unwrap() = Some(body);
        StatusCode::NO_CONTENT
    }
    async fn record_delete(
        State(stub): State<Arc<ShelfStub>>,
        req: axum::http::Request<axum::body::Body>,
    ) -> StatusCode {
        stub.deletes.lock().unwrap().push(req.uri().path().into());
        StatusCode::NO_CONTENT
    }

    let stub = Arc::new(ShelfStub::default());
    let app = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/shelves/preview", post(preview))
        .route("/api/shelves", post(create))
        .route("/api/shelves/{id}", patch(update_forbidden))
        .route("/api/shelves/{id}", delete(record_delete))
        .route("/api/shelves/{id}/books", post(add_books))
        .route("/api/shelves/{id}/books/{uuid}", delete(record_delete))
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

fn candidate_rules() -> Vec<ShelfRule> {
    vec![
        ShelfRule {
            field: RuleField::Author,
            op: RuleOp::Contains,
            value: "le guin".into(),
        },
        ShelfRule {
            field: RuleField::Format,
            op: RuleOp::Includes,
            value: "epub".into(),
        },
    ]
}

#[tokio::test]
async fn preview_shelf_rule_round_trips_the_rule_and_returns_matches_without_creating() {
    let (service, stub) = shelf_stub_service().await;
    let preview = service
        .preview_shelf_rule(Parameters(PreviewRuleParams {
            match_mode: MatchMode::All,
            rules: candidate_rules(),
        }))
        .await
        .unwrap();

    assert_eq!(preview.0.matched, 2);
    assert_eq!(preview.0.total, 10);
    assert_eq!(preview.0.sample.len(), 1);
    assert_eq!(
        preview.0.sample[0].title.as_deref(),
        Some("The Dispossessed")
    );

    // The wire body carried the exact candidate rule, and nothing was created
    // or deleted by previewing.
    let body = stub.last_preview_body.lock().unwrap().clone().unwrap();
    assert_eq!(body["match_mode"], "all");
    assert_eq!(body["rules"][0]["field"], "author");
    assert_eq!(body["rules"][0]["op"], "contains");
    assert_eq!(body["rules"][0]["value"], "le guin");
    assert_eq!(body["rules"][1]["field"], "format");
    assert!(stub.last_create_body.lock().unwrap().is_none());
    assert!(stub.deletes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn create_shelf_sends_the_previewed_rule_as_the_create_payload() {
    let (service, stub) = shelf_stub_service().await;
    let shelf = service
        .create_shelf(Parameters(CreateShelfParams {
            kind: ShelfKind::Smart,
            name: "Le Guin EPUBs".into(),
            description: None,
            visibility: None,
            match_mode: Some(MatchMode::All),
            rules: Some(candidate_rules()),
            book_uuids: None,
        }))
        .await
        .unwrap();
    assert_eq!(shelf.0.id, 5);
    assert_eq!(shelf.0.name, "Le Guin EPUBs");

    let body = stub.last_create_body.lock().unwrap().clone().unwrap();
    assert_eq!(body["kind"], "smart");
    assert_eq!(body["name"], "Le Guin EPUBs");
    assert_eq!(body["visibility"], "private");
    assert_eq!(body["match_mode"], "all");
    assert_eq!(body["rules"][0]["value"], "le guin");
    assert_eq!(body["rules"][1]["value"], "epub");
    assert_eq!(body["book_uuids"], serde_json::json!([]));
}

#[tokio::test]
async fn add_books_to_shelf_posts_the_explicit_uuid_list() {
    let (service, stub) = shelf_stub_service().await;
    let ack = service
        .add_books_to_shelf(Parameters(AddBooksParams {
            id: 5,
            book_uuids: vec!["uuid-a".into(), "uuid-b".into()],
        }))
        .await
        .unwrap();
    assert!(ack.0.done);
    let body = stub.last_add_body.lock().unwrap().clone().unwrap();
    assert_eq!(body["book_uuids"], serde_json::json!(["uuid-a", "uuid-b"]));
}

#[tokio::test]
async fn remove_book_from_shelf_deletes_the_membership_row() {
    let (service, stub) = shelf_stub_service().await;
    let ack = service
        .remove_book_from_shelf(Parameters(RemoveBookParams {
            id: 5,
            uuid: "uuid-a".into(),
        }))
        .await
        .unwrap();
    assert!(ack.0.done);
    assert_eq!(
        stub.deletes.lock().unwrap().clone(),
        vec!["/api/shelves/5/books/uuid-a".to_string()]
    );
}

#[tokio::test]
async fn delete_shelf_refuses_without_explicit_confirmation() {
    let (service, stub) = shelf_stub_service().await;
    let err = match service
        .delete_shelf(Parameters(DeleteShelfParams {
            id: 5,
            confirm: false,
        }))
        .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected a confirmation refusal"),
    };
    assert!(err.message.contains("confirm: true"));
    // The refusal happened before any request left the client.
    assert!(stub.deletes.lock().unwrap().is_empty());
}

#[tokio::test]
async fn delete_shelf_deletes_when_confirmed() {
    let (service, stub) = shelf_stub_service().await;
    let ack = service
        .delete_shelf(Parameters(DeleteShelfParams {
            id: 5,
            confirm: true,
        }))
        .await
        .unwrap();
    assert!(ack.0.done);
    assert_eq!(
        stub.deletes.lock().unwrap().clone(),
        vec!["/api/shelves/5".to_string()]
    );
}

#[tokio::test]
async fn update_shelf_surfaces_a_403_naming_the_ownership_rule() {
    let (service, _stub) = shelf_stub_service().await;
    let err = match service
        .update_shelf(Parameters(UpdateShelfParams {
            id: 9,
            name: Some("Renamed".into()),
            description: None,
            visibility: None,
            match_mode: None,
            rules: None,
            sync_to_kobo: None,
        }))
        .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected a forbidden error"),
    };
    assert!(err.message.contains("owner"), "message: {}", err.message);
    assert!(
        err.message.contains("not your shelf"),
        "message: {}",
        err.message
    );
}

#[tokio::test]
async fn remove_book_from_shelf_rejects_a_uuid_that_is_not_one_path_segment() {
    // A slash-bearing "uuid" would otherwise split the request path into
    // extra segments and trip the WRITE_ALLOWLIST assert — a panic, not an
    // error. The guard must answer invalid params before any request forms.
    let service = offline_server();
    let err = match service
        .remove_book_from_shelf(Parameters(RemoveBookParams {
            id: 5,
            uuid: "uuid-a/../../settings".into(),
        }))
        .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected an invalid-params error"),
    };
    assert!(err.message.contains("uuid"), "message: {}", err.message);
}

#[tokio::test]
async fn create_shelf_rejects_invalid_kind_combinations_locally() {
    // A smart shelf without rules fails CreateShelfRequest::validate before
    // any request leaves the client (offline_server has no instance behind
    // it, so reaching the network would surface a transport error instead).
    let service = offline_server();
    let err = match service
        .create_shelf(Parameters(CreateShelfParams {
            kind: ShelfKind::Smart,
            name: "Broken".into(),
            description: None,
            visibility: None,
            match_mode: None,
            rules: None,
            book_uuids: None,
        }))
        .await
    {
        Err(e) => e,
        Ok(_) => panic!("expected a validation error"),
    };
    assert!(
        err.message.contains("match mode"),
        "message: {}",
        err.message
    );
}
