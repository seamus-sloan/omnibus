use std::sync::{Arc, Mutex};

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::post;
use axum::{Json as AxumJson, Router};
use rmcp::handler::server::wrapper::Parameters;

use omnibus_shared::{MergeBooksResult, UndoMergeResult};

use super::*;
use crate::client::OmnibusClient;
use crate::config::Config;

/// Every merge tool this issue ships, by MCP tool name.
const EXPECTED_TOOLS: &[&str] = &["merge_books", "undo_merge"];

/// Stub instance state: records merge/undo bodies and lets a test force a
/// failure status on either route.
#[derive(Default)]
struct Stub {
    merges: Mutex<Vec<serde_json::Value>>,
    undos: Mutex<Vec<serde_json::Value>>,
    fail: Mutex<Option<(u16, &'static str)>>,
}

async fn login() -> AxumJson<serde_json::Value> {
    AxumJson(serde_json::json!({
        "user": {
            "id": 1, "username": "admin", "is_admin": true,
            "can_upload": true, "can_edit": true, "can_download": true
        },
        "token": "token-1",
    }))
}

async fn post_merge(
    State(stub): State<Arc<Stub>>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> Result<AxumJson<MergeBooksResult>, (StatusCode, &'static str)> {
    if let Some((status, message)) = *stub.fail.lock().unwrap() {
        return Err((StatusCode::from_u16(status).unwrap(), message));
    }
    let target = body["target_uuid"].as_str().unwrap().to_string();
    stub.merges.lock().unwrap().push(body);
    Ok(AxumJson(MergeBooksResult {
        merge_log_id: 42,
        target_uuid: target,
    }))
}

async fn post_undo(
    State(stub): State<Arc<Stub>>,
    AxumJson(body): AxumJson<serde_json::Value>,
) -> Result<AxumJson<UndoMergeResult>, (StatusCode, &'static str)> {
    if let Some((status, message)) = *stub.fail.lock().unwrap() {
        return Err((StatusCode::from_u16(status).unwrap(), message));
    }
    stub.undos.lock().unwrap().push(body);
    Ok(AxumJson(UndoMergeResult {
        restored_uuid: "uuid-source".into(),
    }))
}

/// Boot a stub instance and return a service pointed at it plus the stub.
async fn stub_service() -> (OmnibusMcp, Arc<Stub>) {
    let stub = Arc::new(Stub::default());
    let app = Router::new()
        .route("/api/auth/login", post(login))
        .route("/api/books/merge", post(post_merge))
        .route("/api/books/merge/undo", post(post_undo))
        .with_state(stub.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let client = OmnibusClient::new(Config {
        base_url: format!("http://{addr}"),
        username: "admin".into(),
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

fn merge_params(confirm: Option<bool>) -> MergeParams {
    MergeParams {
        source_uuid: "uuid-source".into(),
        target_uuid: "uuid-target".into(),
        confirm,
    }
}

#[test]
fn merge_tools_router_lists_every_expected_tool_with_a_description() {
    let router = OmnibusMcp::merge_tools();
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
async fn merge_books_refuses_without_confirm_and_sends_nothing() {
    let (service, stub) = stub_service().await;
    for confirm in [None, Some(false)] {
        let err = expect_err(service.merge_books(Parameters(merge_params(confirm))).await);
        assert!(err.message.contains("refused"));
        assert!(err.message.contains("confirm: true"));
        // The workflow: fetch both books, present them, then confirm.
        assert!(err.message.contains("get_book"), "got: {}", err.message);
    }
    assert!(stub.merges.lock().unwrap().is_empty());
}

#[tokio::test]
async fn merge_books_posts_the_pair_and_returns_the_undo_handle_when_confirmed() {
    let (service, stub) = stub_service().await;
    let merged = service
        .merge_books(Parameters(merge_params(Some(true))))
        .await
        .unwrap();
    assert_eq!(merged.0.merge_log_id, 42);
    assert_eq!(merged.0.target_uuid, "uuid-target");
    assert_eq!(
        stub.merges.lock().unwrap().clone(),
        vec![serde_json::json!({
            "source_uuid": "uuid-source",
            "target_uuid": "uuid-target",
        })]
    );
}

#[tokio::test]
async fn merge_books_names_the_admin_requirement_on_a_403() {
    let (service, stub) = stub_service().await;
    *stub.fail.lock().unwrap() = Some((403, "admin required"));
    let err = expect_err(
        service
            .merge_books(Parameters(merge_params(Some(true))))
            .await,
    );
    assert!(err.message.contains("admin"), "got: {}", err.message);
    assert!(
        !err.message.contains("can_edit is required"),
        "must name the admin gate, not can_edit: {}",
        err.message
    );
}

#[tokio::test]
async fn merge_books_rejects_a_uuid_that_is_not_one_path_segment() {
    // A slash-bearing "uuid" in the JSON body cannot split a path here (the
    // merge path is fixed), but the shared guard still rejects it up front so
    // no malformed handle ever reaches the instance.
    let (service, stub) = stub_service().await;
    let err = expect_err(
        service
            .merge_books(Parameters(MergeParams {
                source_uuid: "uuid-a/../uuid-b".into(),
                target_uuid: "uuid-target".into(),
                confirm: Some(true),
            }))
            .await,
    );
    assert!(
        err.message.contains("source_uuid"),
        "message: {}",
        err.message
    );
    assert!(stub.merges.lock().unwrap().is_empty());
}

#[tokio::test]
async fn undo_merge_refuses_without_confirm_and_sends_nothing() {
    let (service, stub) = stub_service().await;
    let err = expect_err(
        service
            .undo_merge(Parameters(UndoMergeParams {
                merge_log_id: 42,
                confirm: None,
            }))
            .await,
    );
    assert!(err.message.contains("refused"));
    assert!(err.message.contains("confirm: true"));
    assert!(stub.undos.lock().unwrap().is_empty());
}

#[tokio::test]
async fn undo_merge_posts_the_log_id_and_returns_the_restored_uuid_when_confirmed() {
    let (service, stub) = stub_service().await;
    let restored = service
        .undo_merge(Parameters(UndoMergeParams {
            merge_log_id: 42,
            confirm: Some(true),
        }))
        .await
        .unwrap();
    assert_eq!(restored.0.restored_uuid, "uuid-source");
    assert_eq!(
        stub.undos.lock().unwrap().clone(),
        vec![serde_json::json!({ "merge_log_id": 42 })]
    );
}

#[tokio::test]
async fn undo_merge_surfaces_the_already_undone_409_message() {
    let (service, stub) = stub_service().await;
    *stub.fail.lock().unwrap() = Some((409, "merge already undone"));
    let err = expect_err(
        service
            .undo_merge(Parameters(UndoMergeParams {
                merge_log_id: 42,
                confirm: Some(true),
            }))
            .await,
    );
    assert!(err.message.contains("HTTP 409"), "got: {}", err.message);
    assert!(err.message.contains("merge already undone"));
}
