use axum::{
    response::{IntoResponse, Response},
    Json,
};

use super::build_id;

/// Unauthenticated liveness + fingerprint endpoint. The `app` field lets
/// `scripts/dev-server-up.sh` distinguish an omnibus instance from some
/// other process that happens to bind the same port. Whitelisted in
/// `auth::gate::require_auth` so it remains reachable without a session.
pub(super) async fn get_health() -> Response {
    Json(serde_json::json!({
        "app": "omnibus",
        "status": "ok",
        "build_id": build_id().to_string(),
    }))
    .into_response()
}
