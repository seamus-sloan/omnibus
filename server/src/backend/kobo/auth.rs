//! Kobo handshake + token exchange: `v1/initialization`, `v1/auth/device`,
//! `v1/auth/refresh`. The device's real credential is the `/kobo/<TOKEN>/`
//! path segment ([`KoboAuthUser`]); these routes hand back a well-formed
//! envelope rather than performing any further verification.

use axum::{
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    Json,
};

use super::{dto, extractor::KoboAuthUser, origin_from_headers, store_resources};

/// Required on the `v1/initialization` response — base64 `{}`. Without it the
/// device treats the payload as malformed and never adopts the resources map.
const KOBO_API_TOKEN: &str = "e30=";

/// `GET v1/initialization` — the handshake that redirects a device at this
/// server. Returns Kobo's own resources map with only the sync/download/cover/
/// annotation entries repointed here, so store browse and search keep working
/// against Kobo directly and this server never proxies that traffic.
///
/// `reading_services_host` points the device's annotation channel here too —
/// answered by [`super::reading_services::reading_services_router`] at the
/// bare origin (#1278), since the device calls it without the path token.
pub async fn initialization(auth: KoboAuthUser, headers: HeaderMap) -> Response {
    let base = origin_from_headers(&headers);
    let resources = store_resources::resources_for(&base, &auth.token);
    (
        StatusCode::OK,
        [(
            header::HeaderName::from_static("x-kobo-apitoken"),
            KOBO_API_TOKEN,
        )],
        Json(serde_json::json!({ "Resources": resources })),
    )
        .into_response()
}

/// `POST v1/auth/device` — the device's initial token exchange. The values are
/// generated locally and never validated afterwards: the `/kobo/<TOKEN>/` path
/// token is the real credential, so this is a well-formed envelope by design,
/// not a stub standing in for verification.
pub async fn auth_device(auth: KoboAuthUser) -> Response {
    Json(dto::auth_envelope(&auth.token)).into_response()
}

/// `POST v1/auth/refresh` — same envelope as [`auth_device`]; the device
/// refreshes on a schedule and expects the same shape back.
pub async fn auth_refresh(auth: KoboAuthUser) -> Response {
    Json(dto::auth_envelope(&auth.token)).into_response()
}
