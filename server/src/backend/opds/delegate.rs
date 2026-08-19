//! Basic-auth'd delegates for the byte-serving links the catalog hands out. An
//! OPDS client replays the catalog's Basic credentials on every link, but the
//! `/api/*` routes those bytes live under are cookie/bearer-only — so feed
//! entries link here, each route resolving [`OpdsAuthUser`] and then calling
//! the `/api` handler body. Range and conditional headers pass through intact.

use axum::{
    extract::{Path, Query, Request, State},
    http::HeaderMap,
    response::Response,
};

use crate::auth::{MediaAuthUser, OpdsAuthUser};

use super::super::{audiobooks, covers, deny_without_download, ebooks};
use super::{guard_shelf_hidden, AppState};

/// `GET /opds/covers/{uuid}` → [`covers::get_cover`].
pub(super) async fn cover(
    user: OpdsAuthUser,
    state: State<AppState>,
    path: Path<String>,
    headers: HeaderMap,
) -> Response {
    if let Some(resp) = guard_shelf_hidden(&state.0, &user, &path.0).await {
        return resp;
    }
    covers::get_cover(MediaAuthUser(user.0), state, path, headers).await
}

/// `GET /opds/thumbs/{uuid}/{size}` → [`covers::get_thumb`].
pub(super) async fn thumb(
    user: OpdsAuthUser,
    state: State<AppState>,
    path: Path<(String, String)>,
    headers: HeaderMap,
) -> Response {
    let (uuid, _size) = &path.0;
    if let Some(resp) = guard_shelf_hidden(&state.0, &user, uuid).await {
        return resp;
    }
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
    if let Some(resp) = guard_shelf_hidden(&state.0, &user, &path.0).await {
        return resp;
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
    if let Some(resp) = guard_shelf_hidden(&state.0, &user, &path.0).await {
        return resp;
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
    if let Some(resp) = guard_shelf_hidden(&state.0, &user, &path.0).await {
        return resp;
    }
    audiobooks::get_audiobook_download(user.0, state, path, query, req).await
}
