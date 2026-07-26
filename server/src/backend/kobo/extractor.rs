//! `KoboAuthUser` — path-token extractor for the wireless Kobo routes.
//!
//! Kobo devices carry their credential in the URL path (`/kobo/<TOKEN>/v1/…`),
//! a channel none of the `/api/*` extractors read. The token is validated as a
//! session token via [`crate::auth::resolve_session_token`].

use axum::{
    extract::FromRequestParts,
    http::{request::Parts, StatusCode},
    response::{IntoResponse, Response},
};
use omnibus_db::auth::SessionAuthError;
use sqlx::SqlitePool;

use crate::auth::{resolve_session_token, AuthUser};

/// Authenticated principal resolved from the `/kobo/<TOKEN>/…` path segment.
/// Carries the raw token too, so handlers can build device-facing absolute
/// URLs (download / image) that echo the same path prefix.
pub struct KoboAuthUser {
    pub user: AuthUser,
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
        match resolve_session_token(&pool, &token).await {
            Ok(user) => Ok(KoboAuthUser { user, token }),
            Err(SessionAuthError::Unauthenticated) => Err(unauthorized()),
            Err(SessionAuthError::Internal(e)) => Err(internal(e)),
        }
    }
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
}

fn internal<E: std::fmt::Display>(e: E) -> Response {
    tracing::error!(error = %e, "kobo auth extractor error");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
}

/// Extract the `<TOKEN>` segment from a `/kobo/<TOKEN>/v1/…` path. Session
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
