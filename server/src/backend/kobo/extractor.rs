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

use crate::http_errors::internal;

/// Authenticated principal resolved from the `/kobo/<TOKEN>/…` path segment:
/// the owning user, the resolved device row, and the raw token (so handlers can
/// build device-facing absolute URLs — download / image — that echo the same
/// path prefix).
pub struct KoboAuthUser {
    pub user_id: i64,
    /// The resolved device row id — keys the per-device sync snapshot (#922).
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
            .ok_or_else(|| {
                internal(
                    "kobo extractor",
                    "missing SqlitePool extension on kobo route",
                )
            })?;

        let resolved = match db::kobo_devices::resolve_device_by_token(&pool, &token).await {
            Ok(Some(r)) => r,
            Ok(None) => return Err(unauthorized()),
            Err(e) => return Err(internal("kobo extractor", e)),
        };

        // Bind the `x-kobo-deviceid` hardware id to this row (steal-on-learn,
        // best-effort — a failure here must not block an otherwise-valid
        // sync). Token-less Reading Services calls authenticate with it.
        if let Some(hw) = hardware_id_header(&parts.headers) {
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

/// Authenticated principal for the token-less Reading Services routes
/// (`/api/v3/...`, #1278): the device speaks at the bare origin, identified
/// only by its `x-kobo-deviceid` header, which a prior tokened
/// `/kobo/<TOKEN>/v1` call bound to a device row. The hardware id is not a
/// secret the way the path token is — it only ever unlocks the annotation
/// channel, and presenting a *token* is what moves it between rows.
pub struct KoboHardwareUser {
    pub user_id: i64,
    pub device_id: i64,
}

impl<S> FromRequestParts<S> for KoboHardwareUser
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let hw = hardware_id_header(&parts.headers)
            .ok_or_else(unauthorized)?
            .to_owned();
        let pool = parts
            .extensions
            .get::<SqlitePool>()
            .cloned()
            .ok_or_else(|| {
                internal(
                    "kobo extractor",
                    "missing SqlitePool extension on reading-services route",
                )
            })?;

        match db::kobo_devices::resolve_device_by_hardware_id(&pool, &hw).await {
            Ok(Some(r)) => Ok(KoboHardwareUser {
                user_id: r.user_id,
                device_id: r.device_id,
            }),
            Ok(None) => Err(unauthorized()),
            Err(e) => Err(internal("kobo extractor", e)),
        }
    }
}

/// Longest `x-kobo-deviceid` value worth a DB roundtrip — real ids are short
/// serial/uuid strings; anything longer is noise or abuse.
const HARDWARE_ID_MAX_LEN: usize = 128;

fn hardware_id_header(headers: &axum::http::HeaderMap) -> Option<&str> {
    headers
        .get("x-kobo-deviceid")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty() && s.len() <= HARDWARE_ID_MAX_LEN)
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
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
