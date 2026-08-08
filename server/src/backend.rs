//! Hand-written `/api/*` REST routes for the mobile client.
//!
//! Web uses Dioxus server functions (see `omnibus_frontend::rpc`), mounted
//! automatically by `dioxus::server::router(App)`. These REST routes are
//! merged alongside them in `main.rs` so mobile's existing `reqwest` paths
//! keep working unchanged.

use std::sync::Arc;

use axum::{
    response::{IntoResponse, Response},
    Extension, Router,
};
use omnibus_db::{
    self as db,
    worker::{Worker, WorkerConfig},
};
use sqlx::SqlitePool;

use crate::http_errors::internal;
use crate::rate_limit::RateLimiter;

mod account;
mod admin_sessions;
mod audiobooks;
mod author_photos;
mod authors;
mod bookmarks;
mod conditional;
mod covers;
mod ebooks;
mod genres;
mod health;
mod highlights;
mod image_upload;
mod journals;
mod kindle;
mod kobo;
mod overrides;
mod physical;
mod process_meta;
mod profile;
mod progress;
mod ratings;
mod read_status;
mod routes;
mod scan;
mod search;
mod series;
mod settings;
mod shelves;
mod stats;
mod suggestions;
mod summary;
mod tags;
mod uploads;
mod users;

pub use kobo::{kobo_router, reading_services_router};
#[cfg(test)]
use process_meta::read_app_version;
pub use process_meta::{
    app_version, build_id, init_app_version, init_build_id, init_repo_root, repo_root,
};

/// Per-IP rate-limit budget for `/api/search/*` and the `/api/rpc/search-*`
/// server functions. Each request runs four FTS5 queries plus joins, so the
/// budget is tighter than the auth-endpoint default (10/60s). 30 requests
/// per 10 seconds per IP comfortably covers a power user typing into the
/// command palette without throttling, while still backstopping a runaway
/// client. Surfaced as a constant so callers / tests share a single source
/// of truth.
pub const SEARCH_RATE_LIMIT_WINDOW: std::time::Duration = std::time::Duration::from_secs(10);
/// Max search requests per [`SEARCH_RATE_LIMIT_WINDOW`] per IP.
pub const SEARCH_RATE_LIMIT_MAX: u32 = 30;

/// Per-IP rate-limit budget for the binary-upload endpoints
/// (`POST /api/ebooks/{id}/cover`, `PUT /api/authors/{id}/photo`,
/// `PUT /api/authors/{id}/photo/url`). Each accepts a multi-MiB image and
/// drives disk I/O, WebP transcoding CPU, and SQLite writes, so a tight
/// loop from a single principal could exhaust resources. 10 uploads per
/// 60 seconds per IP is generous for legitimate editing while still
/// backstopping abuse. Surfaced as constants so callers / tests share a
/// single source of truth.
pub const UPLOAD_RATE_LIMIT_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);
/// Max upload requests per [`UPLOAD_RATE_LIMIT_WINDOW`] per IP.
pub const UPLOAD_RATE_LIMIT_MAX: u32 = 10;

/// Serve a file from disk as a browser download: streamed (so large
/// audiobook files aren't buffered into memory), Range-capable, conditional,
/// and forced to `Content-Disposition: attachment` with the on-disk basename
/// as the suggested filename. `content_type` overrides the guessed MIME.
async fn serve_download(
    req: axum::extract::Request,
    path: &std::path::Path,
    content_type: &str,
) -> Response {
    let filename = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    serve_file(
        req,
        path,
        content_type,
        Some(content_disposition_attachment(filename)),
    )
    .await
}

/// The shared body of every endpoint that streams bytes off disk.
///
/// Opens the file **once** and hands that same handle to both the validator
/// and the stream — see `conditional`'s module doc for why splitting those
/// two is what reopens the splice this whole mechanism exists to close.
/// Every precondition is settled before a byte is read; a missing file is
/// the only condition reported as 404, so a legitimate 416 reaches the
/// client intact with the `Content-Range` it needs to restart.
async fn serve_file(
    req: axum::extract::Request,
    path: &std::path::Path,
    content_type: &str,
    disposition: Option<String>,
) -> Response {
    let open = match conditional::open(path).await {
        Ok(open) => open,
        Err(e) => {
            tracing::warn!(?path, error = %e, "media file open failed");
            return axum::http::StatusCode::NOT_FOUND.into_response();
        }
    };
    let range = match conditional::evaluate(req.headers(), &open.validator, open.len) {
        conditional::Precondition::NotModified(etag) => {
            return conditional::not_modified(
                &etag,
                conditional::MEDIA_CACHE_CONTROL,
                conditional::MEDIA_VARY,
            )
        }
        conditional::Precondition::Failed => {
            return axum::http::StatusCode::PRECONDITION_FAILED.into_response()
        }
        conditional::Precondition::Proceed { range } => range,
    };
    conditional::serve(
        open,
        range,
        conditional::FileResponse {
            content_type,
            cache_control: conditional::MEDIA_CACHE_CONTROL,
            disposition,
        },
    )
    .await
}

/// Build a `Content-Disposition: attachment` value for `filename`.
///
/// Emits both a sanitized ASCII `filename="…"` fallback (control chars,
/// quotes, backslashes, and path separators replaced with `_`) and an
/// RFC 5987 `filename*=UTF-8''…` form so non-ASCII titles survive in
/// browsers that honour it, without pulling in a percent-encoding crate.
fn content_disposition_attachment(filename: &str) -> String {
    let ascii: String = filename
        .chars()
        .map(|c| {
            if c.is_ascii() && !c.is_ascii_control() && !matches!(c, '"' | '\\' | '/') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let mut encoded = String::new();
    for b in filename.as_bytes() {
        // Leave only RFC 3986 "unreserved" bytes (ALPHA / DIGIT / - . _ ~)
        // unescaped — a strict subset of RFC 5987's `attr-char`, so
        // %-encoding everything else is always valid (just more conservative).
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            encoded.push(*b as char);
        } else {
            encoded.push('%');
            encoded.push_str(&format!("{b:02X}"));
        }
    }
    format!("attachment; filename=\"{ascii}\"; filename*=UTF-8''{encoded}")
}

/// Shared axum router state — SQLite pool, worker handle, SSRF guard config.
#[derive(Clone)]
pub struct AppState {
    pool: SqlitePool,
    worker: Arc<Worker>,
    /// SSRF guard config for `put_author_photo_url`. Defaults to strict
    /// (`allow_private_addresses = false`). Integration tests that drive the
    /// handler against a `wiremock` server bound to `127.0.0.1` construct
    /// `AppState` via [`AppState::new_with_remote_image_config`] with the
    /// flag flipped on; production code must never do this.
    remote_image_config: Arc<db::author_photos::RemoteImageConfig>,
}

impl AppState {
    /// Build an `AppState` with the default (strict) SSRF guard.
    pub fn new(pool: SqlitePool) -> Self {
        let worker = Worker::new(pool.clone(), WorkerConfig::default());
        Self {
            pool,
            worker,
            remote_image_config: Arc::new(db::author_photos::RemoteImageConfig::default()),
        }
    }

    /// Test-only constructor that overrides the SSRF guard config. Wraps
    /// [`AppState::new`] so the SQLite pool + worker stay identical and only
    /// the [`db::author_photos::RemoteImageConfig`] field differs. Marked
    /// `#[cfg(test)]` so production callers cannot accidentally flip
    /// `allow_private_addresses`.
    #[cfg(test)]
    pub(crate) fn new_with_remote_image_config(
        pool: SqlitePool,
        cfg: db::author_photos::RemoteImageConfig,
    ) -> Self {
        Self {
            remote_image_config: Arc::new(cfg),
            ..Self::new(pool)
        }
    }

    /// SQLite connection pool used by every handler.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    /// Shared single-process background worker handle.
    pub fn worker(&self) -> &Arc<Worker> {
        &self.worker
    }

    /// SSRF guard config consulted before fetching admin-supplied URLs.
    pub fn remote_image_config(&self) -> &db::author_photos::RemoteImageConfig {
        &self.remote_image_config
    }
}

/// Build the `/api/*` REST router with a fresh per-call search rate-limiter.
pub fn rest_router(state: AppState) -> Router {
    // Standalone/test entrypoint: a fresh, dedicated search limiter per call.
    // The live server uses `rest_router_with_search_limiter` to share one (#249).
    rest_router_with_search_limiter(
        state,
        std::sync::Arc::new(RateLimiter::with_policy(
            SEARCH_RATE_LIMIT_WINDOW,
            SEARCH_RATE_LIMIT_MAX,
        )),
    )
}

/// Build the `/api/*` REST router, sharing `search_limiter` with the caller so
/// REST `/api/search/*` and RPC `/api/rpc/search-*` draw from one budget.
pub fn rest_router_with_search_limiter(
    state: AppState,
    search_limiter: std::sync::Arc<RateLimiter>,
) -> Router {
    let pool = state.pool().clone();
    routes::content_routes()
        .merge(routes::data_routes(search_limiter))
        .with_state(state)
        // `AuthUser`/`AdminUser` read the pool from `Extension<SqlitePool>`.
        // Layer it here so the router is self-contained for integration
        // tests; in the live server `main.rs` adds the same Extension at
        // the top, which is harmless overlap.
        .layer(Extension(pool))
        // Global guards against slow / oversized clients. `main.rs` layers
        // the same protections at the very top so the auth router and
        // Dioxus server functions are covered too; duplicating them here
        // means integration tests (which use `rest_router` directly) also
        // exercise the limits.
        .layer(tower_http::timeout::TimeoutLayer::with_status_code(
            axum::http::StatusCode::REQUEST_TIMEOUT,
            std::time::Duration::from_secs(30),
        ))
        .layer(axum::extract::DefaultBodyLimit::max(1024 * 1024))
}

/// Attach the pagination hint headers to a list/search response.
///
/// * `X-Total-Count` — the true row count of the underlying query, before
///   the `MAX_BOOKS_RETURNED` server-side cap is applied.
/// * `X-Total-Cap` — the cap itself, only set when the response was
///   actually truncated. Clients can branch on its presence to decide
///   whether to surface a "too large to fully load" hint.
///
/// The JSON body shape is intentionally unchanged so older mobile clients
/// keep parsing the response as `EbookLibrary` without any wire-format
/// migration.
fn with_pagination_headers(mut resp: Response, total: i64) -> Response {
    use axum::http::HeaderValue;
    if let Ok(v) = HeaderValue::from_str(&total.to_string()) {
        resp.headers_mut().insert("X-Total-Count", v);
    }
    if total > db::MAX_BOOKS_RETURNED {
        if let Ok(v) = HeaderValue::from_str(&db::MAX_BOOKS_RETURNED.to_string()) {
            resp.headers_mut().insert("X-Total-Cap", v);
        }
    }
    resp
}

#[cfg(test)]
pub(crate) mod test_support;

#[cfg(test)]
mod tests;
