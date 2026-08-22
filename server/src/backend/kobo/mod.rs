//! Native Kobo wireless sync (`/kobo/<TOKEN>/v1/*`), mounted outside the
//! `/api/*` auth gate — each route authenticates via its path token
//! ([`KoboAuthUser`]). Split by protocol phase: [`auth`] (handshake and token
//! exchange), [`sync`] (library sync plus read-state GET), [`state`]
//! (read-state PUT), and [`resources`] (download, cover, tags).

use axum::{
    extract::Path,
    http::{header, HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{any, get, post},
    Extension, Json, Router,
};

use super::AppState;
use crate::http_errors::internal;

mod analytics;
mod auth;
mod dto;
mod extractor;
mod reading_services;
mod resources;
mod state;
mod store_resources;
mod sync;
#[cfg(test)]
mod tests;

use extractor::KoboAuthUser;
pub use reading_services::reading_services_router;

/// Build the wireless Kobo router. `Extension(pool)` is layered here so the
/// router is self-contained for integration tests; the live server adds the
/// same one at the top (harmless overlap, mirroring `rest_router`).
pub fn kobo_router(state: AppState) -> Router {
    let pool = state.pool().clone();
    Router::new()
        .route("/kobo/{token}/v1/initialization", get(auth::initialization))
        .route("/kobo/{token}/v1/auth/device", post(auth::auth_device))
        .route("/kobo/{token}/v1/auth/refresh", post(auth::auth_refresh))
        .route(
            "/kobo/{token}/v1/analytics/event",
            post(analytics::analytics_event),
        )
        // Real firmware POSTs gettests despite it being a fetch; serve both.
        .route(
            "/kobo/{token}/v1/analytics/gettests",
            get(analytics::analytics_gettests).post(analytics::analytics_gettests),
        )
        .route("/kobo/{token}/v1/library/sync", get(sync::library_sync))
        .route(
            "/kobo/{token}/v1/library/{uuid}/metadata",
            get(sync::library_metadata),
        )
        .route(
            "/kobo/{token}/v1/library/{uuid}/state",
            get(sync::get_state).put(state::put_state),
        )
        .route(
            "/kobo/{token}/v1/library/tags",
            get(resources::library_tags),
        )
        .route("/kobo/{token}/v1/download/{uuid}", get(resources::download))
        .route(
            "/kobo/{token}/v1/books/{uuid}/thumbnail/{w}/{h}/{quality}/{greyscale}/image.jpg",
            get(resources::image),
        )
        // Registered routes win over the wildcard; only unhandled paths land here.
        .route("/kobo/{token}/{*rest}", any(store_stub))
        .with_state(state)
        .layer(Extension(pool))
}

/// Benign `200 {}` for store paths the firmware derives from `api_endpoint`
/// itself (`v1/user/profile`, `v1/deals`, …), bypassing the initialization
/// resources map that points them at Kobo. A 404 on any of them makes the
/// device abort the whole sync before `library/sync`; Calibre-Web answers the
/// same paths with an empty object. The log line doubles as capture data for
/// the #928 golden fixture.
async fn store_stub(auth: KoboAuthUser, Path((_token, rest)): Path<(String, String)>) -> Response {
    // `?rest` (Debug) escapes control chars the router percent-decodes into
    // the path; device_id makes multi-device captures attributable.
    tracing::info!(
        device_id = auth.device_id,
        path = ?rest,
        "kobo store path answered with empty stub"
    );
    Json(serde_json::json!({})).into_response()
}

/// Cap a path `{uuid}` at [`omnibus_shared::BOOK_UUID_MAX_LEN`] before any DB
/// round trip, mirroring the request-input sweep the JSON-body routes already
/// follow (`kindle::SendBody::validate`). `Some(response)` is the rejection.
/// Shared across [`sync`] and [`resources`] handlers.
fn reject_oversized_uuid(uuid: &str) -> Option<Response> {
    (uuid.len() > omnibus_shared::BOOK_UUID_MAX_LEN)
        .then(|| StatusCode::BAD_REQUEST.into_response())
}

/// Reconstruct the request origin (`scheme://host`) so download/image URLs are
/// absolute. Prefers `X-Forwarded-Host` (reverse proxy) over `Host`, and
/// `X-Forwarded-Proto` over a `http` default. When no host is resolvable,
/// returns an empty string so callers emit host-relative URLs rather than an
/// invalid `http:///…`. Shared across [`auth`] and [`sync`] handlers.
fn origin_from_headers(headers: &HeaderMap) -> String {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .filter(|h| !h.is_empty());
    let Some(host) = host else {
        return String::new();
    };
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("http");
    format!("{scheme}://{host}")
}
