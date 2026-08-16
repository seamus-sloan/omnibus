//! Basic-auth'd delegates for the byte-serving links the catalog hands
//! out. An OPDS client replays the catalog's Basic credentials on every
//! link it follows, but the `/api/*` routes those bytes live under are
//! cookie/bearer-only — so feed entries link here instead, and each route
//! resolves [`OpdsAuthUser`] then calls the `/api` handler body with the
//! resolved user. Query strings and the raw request (range/conditional
//! headers) pass through untouched, so resume and 304 behaviour is
//! identical to the `/api` originals.
//!
//! Each route also re-checks [`super::guard_shelf_hidden`] (#932) before
//! delegating: a book that's confined to a manual shelf this viewer can't
//! see must not be fetchable by a client that already knows (or guesses)
//! its uuid, even though it never surfaced in a feed. Answered as a 404,
//! matching `shelves::load_visible_shelf`'s no-existence-leak rule.

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
