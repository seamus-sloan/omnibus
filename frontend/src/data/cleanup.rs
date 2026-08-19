//! Library-cleanup review client wrappers: per-kind counts, the pending
//! queue, the decide/detect/undo/delete-entity writes. Web/SSR call the
//! `rpc_cleanup_*` server functions; mobile posts the same `/api/rpc/cleanup/*`
//! routes over `reqwest`, since the admin cleanup surface has no separate REST
//! twin. No mobile UI ships yet — the transport exists so the data layer keeps
//! compiling under the mobile gate.

use omnibus_shared::{CleanupCounts, CleanupKind, Decision, SuggestionCard};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// How many pending cards the review page asks for in one go. Matches the
/// server's own clamp so the client never asks for a page it can't get.
pub const CLEANUP_QUEUE_PAGE: i64 = 50;

/// Mobile: POST one `/api/rpc/cleanup/*` route with a JSON argument object and
/// decode the JSON result. The server functions take their arguments as a
/// single JSON object keyed by parameter name, so this is the same wire
/// contract the web client's generated stubs use.
#[cfg(feature = "mobile")]
async fn post_cleanup<T: serde::de::DeserializeOwned>(
    server_url: &str,
    path: &str,
    body: serde_json::Value,
) -> Result<T, DataError> {
    crate::data::require_online()?;
    let url = format!("{server_url}/api/rpc/cleanup/{path}");
    let response = with_bearer(http_client().post(&url))
        .json(&body)
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<T>().await?)
}

/// Web/SSR: per-kind pending/accepted/rejected counts for the Settings section.
#[cfg(not(feature = "mobile"))]
pub async fn get_cleanup_counts(
    _server_url: &str,
) -> Result<Vec<(CleanupKind, CleanupCounts)>, DataError> {
    crate::rpc::rpc_cleanup_counts()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/rpc/cleanup/counts`.
#[cfg(feature = "mobile")]
pub async fn get_cleanup_counts(
    server_url: &str,
) -> Result<Vec<(CleanupKind, CleanupCounts)>, DataError> {
    post_cleanup(server_url, "counts", serde_json::json!({})).await
}

/// Web/SSR: the pending review queue for one kind, oldest first.
#[cfg(not(feature = "mobile"))]
pub async fn get_cleanup_queue(
    _server_url: &str,
    kind: CleanupKind,
    limit: i64,
) -> Result<Vec<SuggestionCard>, DataError> {
    crate::rpc::rpc_cleanup_queue(kind, limit)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/rpc/cleanup/queue`.
#[cfg(feature = "mobile")]
pub async fn get_cleanup_queue(
    server_url: &str,
    kind: CleanupKind,
    limit: i64,
) -> Result<Vec<SuggestionCard>, DataError> {
    post_cleanup(
        server_url,
        "queue",
        serde_json::json!({ "kind": kind, "limit": limit }),
    )
    .await
}

/// Web/SSR: record an Accept / Reject on one suggestion. Accepting also runs
/// the apply primitive server-side, so this returns only once the library has
/// actually changed.
#[cfg(not(feature = "mobile"))]
pub async fn decide_cleanup_suggestion(
    _server_url: &str,
    id: i64,
    decision: Decision,
) -> Result<(), DataError> {
    crate::rpc::rpc_cleanup_decide(id, decision)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/rpc/cleanup/decide`. Never queued offline — accepting
/// applies a library-wide taxonomy change every user sees, which is exactly
/// what `.claude/rules/08-offline-writes.md`'s first test excludes.
#[cfg(feature = "mobile")]
pub async fn decide_cleanup_suggestion(
    server_url: &str,
    id: i64,
    decision: Decision,
) -> Result<(), DataError> {
    post_cleanup(
        server_url,
        "decide",
        serde_json::json!({ "id": id, "decision": decision }),
    )
    .await
}

/// Web/SSR: queue a detection pass (`None` runs every detector), returning the
/// worker task id.
#[cfg(not(feature = "mobile"))]
pub async fn run_cleanup_detection(
    _server_url: &str,
    kind: Option<CleanupKind>,
) -> Result<u64, DataError> {
    crate::rpc::rpc_cleanup_detect(kind)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/rpc/cleanup/detect`.
#[cfg(feature = "mobile")]
pub async fn run_cleanup_detection(
    server_url: &str,
    kind: Option<CleanupKind>,
) -> Result<u64, DataError> {
    post_cleanup(server_url, "detect", serde_json::json!({ "kind": kind })).await
}

/// Web/SSR: reverse an applied cleanup by its `cleanup_log` id.
#[cfg(not(feature = "mobile"))]
pub async fn undo_cleanup(_server_url: &str, log_id: i64) -> Result<(), DataError> {
    crate::rpc::rpc_cleanup_undo(log_id)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/rpc/cleanup/undo`.
#[cfg(feature = "mobile")]
pub async fn undo_cleanup(server_url: &str, log_id: i64) -> Result<(), DataError> {
    post_cleanup(server_url, "undo", serde_json::json!({ "log_id": log_id })).await
}

/// Web/SSR: delete a taxonomy entity outright, returning the `cleanup_log` id
/// an undo would replay from.
#[cfg(not(feature = "mobile"))]
pub async fn delete_cleanup_entity(
    _server_url: &str,
    kind: CleanupKind,
    entity_id: i64,
) -> Result<i64, DataError> {
    crate::rpc::rpc_cleanup_delete_entity(kind, entity_id)
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: POST `/api/rpc/cleanup/delete-entity`.
#[cfg(feature = "mobile")]
pub async fn delete_cleanup_entity(
    server_url: &str,
    kind: CleanupKind,
    entity_id: i64,
) -> Result<i64, DataError> {
    post_cleanup(
        server_url,
        "delete-entity",
        serde_json::json!({ "kind": kind, "entity_id": entity_id }),
    )
    .await
}
