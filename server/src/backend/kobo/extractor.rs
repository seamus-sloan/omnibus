//! `KoboAuthUser` — path-token extractor for the wireless Kobo routes.
//!
//! Kobo devices carry their credential in the URL path (`/kobo/<TOKEN>/v1/…`),
//! a channel none of the `/api/*` extractors read. The token is a persistent,
//! per-device `kobo_devices` credential (not a session token), resolved via
//! [`db::kobo_devices::resolve_device_by_token`].

use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
};
use omnibus_db as db;
use sqlx::SqlitePool;

/// Authenticated principal resolved from the `/kobo/<TOKEN>/…` path segment:
/// the owning user, the resolved device row, and the raw token (so handlers can
/// build device-facing absolute URLs — download / image — that echo the same
/// path prefix).
pub struct KoboAuthUser {
    pub user_id: i64,
    /// The resolved device row id. Not yet read by a handler — it's the hook
    /// the per-device delta cursor (#926) hangs off — so allow it to be unused.
    #[allow(dead_code)]
    pub device_id: i64,
    pub token: String,
}

impl<S> FromRequestParts<S> for KoboAuthUser
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let token = kobo_path_token(parts.uri.path())
            .ok_or_else(unauthorized)?
            .to_owned();
        let pool = parts
            .extensions
            .get::<SqlitePool>()
            .cloned()
            .ok_or_else(|| internal("missing SqlitePool extension on kobo route"))?;

        let resolved = match db::kobo_devices::resolve_device_by_token(&pool, &token).await {
            Ok(Some(r)) => r,
            Ok(None) => return Err(unauthorized()),
            Err(e) => return Err(internal(e)),
        };

        // Learn the `x-kobo-deviceid` hardware id on first sight (write-once,
        // best-effort — a failure here must not block an otherwise-valid sync).
        if let Some(hw) = parts
            .headers
            .get("x-kobo-deviceid")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.is_empty())
        {
            if let Err(e) =
                db::kobo_devices::learn_kobo_device_id(&pool, resolved.device_id, hw).await
            {
                tracing::warn!(error = %e, "failed to persist x-kobo-deviceid");
            }
        }

        Ok(KoboAuthUser {
            user_id: resolved.user_id,
            device_id: resolved.device_id,
            token,
        })
    }
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
}

fn internal<E: std::fmt::Display>(e: E) -> Response {
    tracing::error!(error = %e, "kobo auth extractor error");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
}

/// Extract the `<TOKEN>` segment from a `/kobo/<TOKEN>/v1/…` path. Kobo device
/// tokens are URL-safe base64 (no `/`), so plain segment splitting is safe.
fn kobo_path_token(path: &str) -> Option<&str> {
    let mut segs = path.split('/').filter(|s| !s.is_empty());
    (segs.next() == Some("kobo")).then(|| segs.next()).flatten()
}

#[cfg(test)]
mod tests {
    use super::kobo_path_token;

    #[test]
    fn kobo_path_token_reads_the_segment_after_kobo() {
        assert_eq!(
            kobo_path_token("/kobo/abc123/v1/library/sync"),
            Some("abc123")
        );
        assert_eq!(
            kobo_path_token("/kobo/tok/v1/library/uuid/state"),
            Some("tok")
        );
    }

    #[test]
    fn kobo_path_token_rejects_non_kobo_paths() {
        assert_eq!(kobo_path_token("/api/ebooks"), None);
        assert_eq!(kobo_path_token("/kobo"), None);
        assert_eq!(kobo_path_token("/"), None);
    }
}
