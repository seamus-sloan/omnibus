//! `require_auth` — top-level middleware, applied in `server/src/main.rs`,
//! that gates `/api/*` routes behind a live session. Everything under
//! `/api/*` other than `/api/auth/*` and `/api/_health` requires a valid
//! session or gets `401 Unauthorized`; everything else (SSR HTML, WASM
//! bundle, static assets) passes through untouched. Media read endpoints
//! (covers, thumbs, author-photo / suggestion-cover GETs, and direct-play
//! audiobook part streams) additionally accept the session as a `?token=`
//! query param — see [`is_media_read_path`].

use axum::{
    extract::{Request, State},
    http::{Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use omnibus_db::auth as auth_db;

use super::extractor::{extract_token, query_token};
use crate::backend::AppState;

/// Media read endpoints whose bytes are rendered into an `<img src>` (covers,
/// thumbs, photos) or `<audio src>` (direct-play audiobook parts) and so may
/// authenticate via a `?token=` query param — the only auth such a WebView
/// media fetch can carry. Kept in lockstep with the handlers gated on
/// [`MediaAuthUser`]; every other `/api/*` path stays header/cookie-only.
///
/// [`MediaAuthUser`]: super::MediaAuthUser
fn is_media_read_path(path: &str) -> bool {
    path.starts_with("/api/covers/")
        || path.starts_with("/api/thumbs/")
        || (path.starts_with("/api/authors/") && path.ends_with("/photo"))
        || (path.starts_with("/api/suggestions/") && path.ends_with("/cover"))
        // Direct-play audiobook part streams feed the mobile WebView's
        // `<audio src>`, which can only carry the session as `?token=`. HLS
        // segments are web-only (cookie-authed) so they stay off this list.
        || (path.starts_with("/api/audiobooks/") && path.contains("/parts/"))
        // The EPUB file stream is fetched by epub.js from the mobile WebView,
        // a cross-origin XHR that carries neither cookie nor bearer header.
        // Only `/file` opts in; `/download` and `/kepub` stay header/cookie-only.
        || (path.starts_with("/api/ebooks/") && path.ends_with("/file"))
        // Embedded journal images render into `<img src>` in the mobile
        // WebView's journal feed — same fetch constraints as covers/thumbs.
        || path.starts_with("/api/journals/images/")
}

/// Reject `/api/*` requests without a live session with `401 Unauthorized`, after exempting `/api/auth/*` and `/api/_health`.
///
/// `/api/auth/*` handles its own authentication (`/me` uses the
/// [`AuthUser`](super::AuthUser) extractor; `/login`/`/register` deliberately
/// don't require auth). `/api/_health` is an unauthenticated liveness +
/// fingerprint probe used by `scripts/dev-server-up.sh` to decide whether to
/// reuse an existing omnibus server on the port — it cannot require auth or
/// the probe couldn't run before a session exists. This middleware does not
/// set per-user request extensions: handlers that need `AuthUser` should
/// still declare it as an extractor; this only gates the *boundary*.
pub async fn require_auth(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let path = req.uri().path();
    if !path.starts_with("/api/")
        || path == "/api/auth"
        || path.starts_with("/api/auth/")
        || path == "/api/_health"
    {
        return next.run(req).await;
    }
    // Header/cookie is the norm; a GET on a media read path may instead carry
    // the session as `?token=` so a mobile WebView `<img>` fetch authenticates.
    let is_media = req.method() == Method::GET && is_media_read_path(path);
    let header_token = extract_token(req.headers()).map(|(t, _)| t);
    let token = header_token.or_else(|| {
        if is_media {
            let qt = query_token(req.uri().query());
            if qt.is_some() {
                tracing::debug!(path, "media auth: using ?token= query param");
            } else {
                tracing::warn!(
                    path,
                    "media auth: no bearer header, cookie, or ?token= — will 401"
                );
            }
            qt
        } else {
            None
        }
    });
    let Some(token) = token else {
        return (StatusCode::UNAUTHORIZED, "unauthorized").into_response();
    };
    match auth_db::lookup_session(state.pool(), &token).await {
        Ok(_) => next.run(req).await,
        Err(auth_db::AuthError::SessionNotFound) => {
            if is_media {
                tracing::warn!(
                    path,
                    "media auth: token not found in session store — will 401"
                );
            }
            (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
        }
        Err(e) => {
            tracing::error!(error = %e, "require_auth: session lookup failed");
            (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
        }
    }
}

#[cfg(test)]
mod tests;
