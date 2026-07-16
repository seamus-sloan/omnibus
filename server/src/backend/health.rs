//! `GET /api/_health` handler.
//!
//! Unauthenticated liveness + build-fingerprint endpoint used by
//! `scripts/dev-server-up.sh` to identify a running omnibus instance and
//! its worktree. Whitelisted in `auth::gate::require_auth`.

use axum::{
    response::{IntoResponse, Response},
    Json,
};

use super::{app_version, build_id, repo_root};

/// Unauthenticated liveness + fingerprint endpoint. The `app` field lets
/// `scripts/dev-server-up.sh` distinguish an omnibus instance from some
/// other process that happens to bind the same port. The `repo_root`
/// field lets it distinguish *this* workspace's server from a sibling
/// `jj` workspace's server bound to the same port. The `version` field
/// (from `OMNIBUS_VERSION`, `"dev"` when unset) lets the mobile "You"
/// screen show the running server's release alongside its own app version.
/// Whitelisted in `auth::gate::require_auth` so it remains reachable
/// without a session.
pub(super) async fn get_health() -> Response {
    Json(serde_json::json!({
        "app": "omnibus",
        "status": "ok",
        "build_id": build_id().to_string(),
        "repo_root": repo_root(),
        "version": app_version(),
    }))
    .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;
    use serde_json::Value;

    #[tokio::test]
    async fn health_payload_includes_app_status_build_id_and_repo_root() {
        // Drive the handler directly so we don't depend on the router /
        // AppState wiring — this asserts the SHAPE of the JSON payload,
        // which is the contract scripts/dev-server-up.sh parses.
        let response = get_health().await;
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let json: Value = serde_json::from_slice(&body).expect("valid JSON body");

        assert_eq!(json["app"], "omnibus", "app must be 'omnibus'");
        assert_eq!(json["status"], "ok", "status must be 'ok'");

        let build_id = json["build_id"]
            .as_str()
            .expect("build_id is a string of digits");
        assert!(
            build_id.chars().all(|c| c.is_ascii_digit()),
            "build_id should be all-digits, got {build_id:?}",
        );

        let repo_root = json["repo_root"]
            .as_str()
            .expect("repo_root must be a string");
        // OnceLock initializes lazily on first read; we don't assert a
        // specific path because tests don't control cwd, but it must be
        // present and (when populated) an absolute path. Either an empty
        // string (cwd lookup failed — shouldn't happen in test) or a
        // leading `/` is acceptable; the contract is "string field
        // exists" so dev-server-up.sh can parse it.
        assert!(
            repo_root.is_empty() || repo_root.starts_with('/'),
            "repo_root should be absolute or empty, got {repo_root:?}",
        );

        // The test binary doesn't set OMNIBUS_VERSION, so this is "dev" in
        // practice — but the contract this endpoint guarantees is just
        // "a non-empty string field", which is what `read_app_version`'s
        // dedicated fallback test in `backend/tests.rs` covers precisely.
        let version = json["version"].as_str().expect("version is a string");
        assert!(!version.is_empty(), "version should not be empty");
    }
}
