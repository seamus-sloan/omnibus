//! `/api/account/profile`, `/api/account/avatar`, `/api/users/{id}/avatar` —
//! the caller's own display name and profile picture.
//!
//! Writes are scoped to the authenticated caller ([`AuthUser`]) and never take
//! a user id, so there is no path for one user to edit another's profile.
//! Reads are [`MediaAuthUser`]-gated and *are* id-keyed: a journal byline or a
//! ratings row renders any user's avatar.

use axum::{
    extract::{Path, State},
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};
use serde::Deserialize;

use super::conditional::{content_etag, MEDIA_CACHE_CONTROL, MEDIA_VARY};
use super::image_upload::extract_validated_image;
use super::{internal, AppState};
use crate::auth::{AuthUser, MediaAuthUser};

/// Body for `POST /api/account/profile`. A `null`/absent/blank name clears the
/// display name, falling attribution back to the username.
#[derive(Debug, Deserialize)]
pub(super) struct SetProfile {
    #[serde(default)]
    display_name: Option<String>,
}

/// Set or clear the authenticated user's display name. 422 on a name that is
/// too long or carries control characters.
pub(super) async fn post_profile(
    user: AuthUser,
    State(state): State<AppState>,
    Json(body): Json<SetProfile>,
) -> Response {
    match db::auth::set_display_name(&state.pool, user.id, body.display_name.as_deref()).await {
        Ok(()) => StatusCode::OK.into_response(),
        Err(db::auth::AuthError::Validation(msg)) => {
            (StatusCode::UNPROCESSABLE_ENTITY, msg).into_response()
        }
        Err(e) => internal("set display name", e),
    }
}

/// Upload the caller's avatar, replacing any existing one. Multipart with one
/// `avatar` field, through the shared image validation (10 MiB cap, magic-byte
/// sniff, SVG rejected).
pub(super) async fn post_avatar(
    user: AuthUser,
    State(state): State<AppState>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    let (mime, bytes) = match extract_validated_image(&mut multipart, "avatar").await {
        Ok(pair) => pair,
        Err(response) => return response,
    };
    match db::auth::upsert_user_avatar(&state.pool, user.id, &mime, &bytes).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal("upsert user avatar", e),
    }
}

/// Remove the caller's avatar, reverting them to a monogram everywhere.
pub(super) async fn delete_avatar(user: AuthUser, State(state): State<AppState>) -> Response {
    match db::auth::delete_user_avatar(&state.pool, user.id).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal("delete user avatar", e),
    }
}

/// Serve a user's avatar bytes. Byte-derived `ETag` with `If-None-Match`
/// revalidation: the URL is stable per user and the bytes are replaced in
/// place, so a validator is what tells a client its cached copy went stale.
/// `MEDIA_VARY` rides both the 200 and the 304 — the route authenticates by
/// cookie, bearer, *or* `?token=`, so a shared cache must not hand one user's
/// response to a differently-authenticated request on the same URL.
pub(super) async fn get_user_avatar(
    _user: MediaAuthUser,
    State(state): State<AppState>,
    Path(user_id): Path<i64>,
    headers: HeaderMap,
) -> Response {
    match db::auth::get_user_avatar(&state.pool, user_id).await {
        Ok(Some(avatar)) => {
            let etag = content_etag(&avatar.bytes);
            if if_none_match_hits(&headers, &etag) {
                return super::conditional::not_modified(&etag, MEDIA_CACHE_CONTROL, MEDIA_VARY);
            }
            (
                [
                    (header::CONTENT_TYPE, avatar.mime.as_str()),
                    (header::CACHE_CONTROL, MEDIA_CACHE_CONTROL),
                    (header::ETAG, etag.as_str()),
                    (header::VARY, MEDIA_VARY),
                    // An avatar is user-supplied; never let a browser sniff it
                    // into an executable type.
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                ],
                avatar.bytes,
            )
                .into_response()
        }
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(e) => internal("read user avatar", e),
    }
}

/// Whether the request's `If-None-Match` already carries the current `etag`.
fn if_none_match_hits(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
        .is_some_and(|v| super::conditional::if_none_match_hits(v, etag))
}

#[cfg(test)]
mod tests;
