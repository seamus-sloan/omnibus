//! Basic-auth'd delegates for the byte-serving links the catalog hands
//! out. An OPDS client replays the catalog's Basic credentials on every
//! link it follows, but the `/api/*` routes those bytes live under are
//! cookie/bearer-only — so feed entries link here instead, and each route
//! resolves [`OpdsAuthUser`] then calls the `/api` handler body with the
//! resolved user. Query strings and the raw request (range/conditional
//! headers) pass through untouched, so resume and 304 behaviour is
//! identical to the `/api` originals.

use axum::{
    extract::{Path, Query, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};

use crate::auth::{AuthUser, MediaAuthUser, OpdsAuthUser};

use super::super::{audiobooks, covers, ebooks};
use super::AppState;

/// 403 unless the user may download. Applied to the acquisition routes
/// only — covers/thumbs stay open to any authenticated user because the
/// catalog is unrenderable without them, matching the browse UI. (The
/// `/api/*` download originals predate `can_download` and don't enforce
/// it yet — tracked separately.)
fn deny_without_download(user: &AuthUser) -> Option<Response> {
    (!user.can_download).then(|| (StatusCode::FORBIDDEN, "download not permitted").into_response())
}

/// `GET /opds/covers/{uuid}` → [`covers::get_cover`].
pub(super) async fn cover(
    user: OpdsAuthUser,
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
) -> Response {
    covers::get_cover(MediaAuthUser(user.0), state, path, headers).await
}

/// `GET /opds/thumbs/{uuid}/{size}` → [`covers::get_thumb`].
pub(super) async fn thumb(
    user: OpdsAuthUser,
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    covers::get_thumb(MediaAuthUser(user.0), state, path, headers).await
}

/// `GET /opds/ebooks/{uuid}/file` → [`ebooks::get_ebook_file`] (EPUB, else
/// CBZ for a comic-only book).
pub(super) async fn ebook_file(
    user: OpdsAuthUser,
    state: State<AppState>,
    path: Path<String>,
    query: Query<ebooks::EbookFileQuery>,
    req: Request,
) -> Response {
    if let Some(denied) = deny_without_download(&user.0) {
        return denied;
    }
    ebooks::get_ebook_file(MediaAuthUser(user.0), state, path, query, req).await
}

/// `GET /opds/ebooks/{uuid}/download` → [`ebooks::get_ebook_download`]
/// (attachment disposition, overrides baked in).
pub(super) async fn ebook_download(
    user: OpdsAuthUser,
    state: State<AppState>,
    path: Path<String>,
    query: Query<ebooks::EbookFileQuery>,
    req: Request,
) -> Response {
    if let Some(denied) = deny_without_download(&user.0) {
        return denied;
    }
    ebooks::get_ebook_download(user.0, state, path, query, req).await
}

/// `GET /opds/audiobooks/{uuid}/download` → [`audiobooks::get_audiobook_download`].
pub(super) async fn audiobook_download(
    user: OpdsAuthUser,
    state: State<AppState>,
    path: Path<String>,
    query: Query<audiobooks::DownloadQuery>,
    req: Request,
) -> Response {
    if let Some(denied) = deny_without_download(&user.0) {
        return denied;
    }
    audiobooks::get_audiobook_download(user.0, state, path, query, req).await
}
