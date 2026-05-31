//! Hand-written `/api/*` REST routes for the mobile client.
//!
//! Web uses Dioxus server functions (see `omnibus_frontend::rpc`), mounted
//! automatically by `dioxus::server::router(App)`. These REST routes are
//! merged alongside them in `main.rs` so mobile's existing `reqwest` paths
//! keep working unchanged.

use std::sync::Arc;

use axum::{
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Extension, Router,
};
use omnibus_db::{
    self as db,
    worker::{Worker, WorkerConfig},
};
use sqlx::SqlitePool;

use crate::rate_limit::{rate_limit_by_ip, RateLimiter};

mod author_photos;
mod authors;
mod covers;
mod ebooks;
mod health;
mod overrides;
mod progress;
mod search;
mod series;
mod settings;
mod tags;

/// Per-IP rate-limit budget for `/api/search/*` and the `/api/rpc/search-*`
/// server functions. Each request runs four FTS5 queries plus joins, so the
/// budget is tighter than the auth-endpoint default (10/60s). 30 requests
/// per 10 seconds per IP comfortably covers a power user typing into the
/// command palette without throttling, while still backstopping a runaway
/// client. Surfaced as a constant so callers / tests share a single source
/// of truth.
pub const SEARCH_RATE_LIMIT_WINDOW: std::time::Duration = std::time::Duration::from_secs(10);
pub const SEARCH_RATE_LIMIT_MAX: u32 = 30;

/// Per-IP rate-limit budget for the binary-upload endpoints
/// (`POST /api/ebooks/{id}/cover`, `PUT /api/authors/{id}/photo`,
/// `PUT /api/authors/{id}/photo/url`). Each accepts a multi-MiB image and
/// drives disk I/O, WebP transcoding CPU, and SQLite writes, so a tight loop
/// from a single principal could exhaust resources (#168). 10 uploads per
/// 60 seconds per IP is generous for legitimate editing while still
/// backstopping abuse. Surfaced as constants so callers / tests share a
/// single source of truth.
pub const UPLOAD_RATE_LIMIT_WINDOW: std::time::Duration = std::time::Duration::from_secs(60);
pub const UPLOAD_RATE_LIMIT_MAX: u32 = 10;

/// Generic 500 response that never leaks internal error details to the wire.
/// The full error is logged server-side via `tracing::error!` so it remains
/// available in structured logs; the client sees only the boilerplate body.
fn internal<E: std::fmt::Display>(context: &'static str, e: E) -> Response {
    tracing::error!(error = %e, context = context, "internal server error");
    (
        axum::http::StatusCode::INTERNAL_SERVER_ERROR,
        "internal server error",
    )
        .into_response()
}

#[derive(Clone)]
pub struct AppState {
    pool: SqlitePool,
    worker: Arc<Worker>,
    /// Issue #275 SSRF guard config for `put_author_photo_url`. Defaults to
    /// strict (`allow_private_addresses = false`). Integration tests that
    /// drive the handler against a `wiremock` server bound to `127.0.0.1`
    /// construct `AppState` via [`AppState::new_with_remote_image_config`]
    /// with the flag flipped on; production code must never do this.
    remote_image_config: Arc<db::author_photos::RemoteImageConfig>,
}

impl AppState {
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
    /// `allow_private_addresses` (#275).
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

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn worker(&self) -> &Arc<Worker> {
        &self.worker
    }

    pub fn remote_image_config(&self) -> &db::author_photos::RemoteImageConfig {
        &self.remote_image_config
    }
}

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
/// REST `/api/search/*` and RPC `/api/rpc/search-*` draw from one budget (#249).
pub fn rest_router_with_search_limiter(
    state: AppState,
    search_limiter: std::sync::Arc<RateLimiter>,
) -> Router {
    let pool = state.pool().clone();
    Router::new()
        .route("/api/_health", get(health::get_health))
        .route("/api/settings", get(settings::get_settings))
        .route("/api/settings", post(settings::post_settings))
        .route("/api/reindex", post(settings::post_reindex))
        .route("/api/library", get(ebooks::get_library))
        .route("/api/ebooks", get(ebooks::get_ebooks))
        .route("/api/ebooks/{uuid}", get(ebooks::get_ebook_by_uuid))
        .route("/api/ebooks/{uuid}/file", get(ebooks::get_ebook_file))
        .route(
            "/api/ebooks/{uuid}/overrides",
            post(overrides::post_ebook_overrides).delete(overrides::delete_ebook_overrides),
        )
        // F2.1 progress sync — mobile-facing REST. Web hits the analogous
        // `/api/rpc/progress*` server functions defined in `omnibus_frontend::rpc`.
        .route("/api/progress", post(progress::post_progress))
        .route("/api/progress/sessions", post(progress::post_sessions))
        .route("/api/progress/{uuid}", get(progress::get_progress))
        // GET/DELETE for author photos carry no upload body (DELETE mutates,
        // but cheaply — it clears photo state, it doesn't ingest one), so
        // they stay outside the rate-limited `upload_router`. Only the binary
        // uploads (cover POST, photo PUT, photo-url PUT) carry the per-IP
        // frequency cap — see `upload_router` (#168).
        .route(
            "/api/authors/{id}/photo",
            get(author_photos::get_author_photo).delete(author_photos::delete_author_photo),
        )
        .merge(upload_router())
        .merge(search_router(search_limiter))
        .route("/api/covers/{uuid}", get(covers::get_cover))
        .route("/api/thumbs/{uuid}/{size}", get(covers::get_thumb))
        .route("/api/authors", get(authors::get_authors))
        .route(
            "/api/authors/{id}/photo/scan",
            post(author_photos::post_author_photo_scan),
        )
        .route("/api/authors/{id}", get(authors::get_author_by_id))
        .route("/api/series", get(series::get_series))
        .route("/api/series/{id}", get(series::get_series_by_id))
        .route("/api/tags", get(tags::get_tags))
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

/// Sub-router for `/api/search/*` carrying its own per-IP rate-limit layer.
///
/// Each handler runs heavy SQL (four FTS5 queries for the palette; full
/// search for `/api/search`), so we cap each principal at
/// `SEARCH_RATE_LIMIT_MAX` requests per `SEARCH_RATE_LIMIT_WINDOW`. The
/// limiter is constructed per-router so each test (which builds a fresh
/// `rest_router`) gets its own fresh bucket map — production runs through
/// `main.rs::rest_router` once, so the limiter persists for the lifetime
/// of the process.
///
/// Returns `Router<AppState>` so it can be merged into `rest_router` before
/// the outer `.with_state(state)` finalizes the state type. The rate-limit
/// middleware carries its own state (`Arc<RateLimiter>`) via
/// `from_fn_with_state`, which doesn't propagate to the route handlers.
///
/// The `limiter` is passed in so the live server can share one `Arc` across both
/// search families (#249); `rest_router` supplies a fresh dedicated one.
fn search_router(limiter: std::sync::Arc<RateLimiter>) -> Router<AppState> {
    Router::new()
        .route("/api/search", get(search::get_search))
        .route("/api/search/palette", get(search::get_search_palette))
        .layer(axum::middleware::from_fn_with_state(
            limiter,
            rate_limit_by_ip,
        ))
}

/// Per-IP rate-limited router for the binary-upload endpoints. Mirrors
/// [`search_router`]: a dedicated sub-router carrying its own
/// `Arc<RateLimiter>` via `from_fn_with_state` (state that doesn't propagate
/// to the handlers), merged into [`rest_router`]. The three routes — cover
/// `POST`, author-photo `PUT`, author-photo-URL `PUT` — each accept a
/// multi-MiB payload and drive disk I/O, WebP transcoding, and SQLite
/// writes, so they share a per-IP budget (#168). The `GET`/`DELETE` author-
/// photo routes carry no upload body (DELETE mutates but cheaply), so they
/// stay in `rest_router`, outside this limiter.
fn upload_router() -> Router<AppState> {
    let limiter = std::sync::Arc::new(RateLimiter::with_policy(
        UPLOAD_RATE_LIMIT_WINDOW,
        UPLOAD_RATE_LIMIT_MAX,
    ));
    Router::new()
        .route(
            "/api/ebooks/{uuid}/cover",
            post(overrides::post_ebook_cover),
        )
        .route(
            "/api/authors/{id}/photo",
            put(author_photos::put_author_photo),
        )
        .route(
            "/api/authors/{id}/photo/url",
            put(author_photos::put_author_photo_url),
        )
        // Image uploads need more than the global 1 MiB cap; layered closer
        // to the handler than the global limit in `main.rs`, so axum picks
        // this larger value for these routes (preserves the prior 11 MiB cap
        // that was on the cover/photo routes).
        .layer(axum::extract::DefaultBodyLimit::max(11 * 1024 * 1024))
        // Rate-limit layer added last so it is outermost: an over-budget
        // request is rejected before the body is buffered or the handler runs.
        .layer(axum::middleware::from_fn_with_state(
            limiter,
            rate_limit_by_ip,
        ))
}

/// Process-start build id. Captured once and preserved for the lifetime of
/// the process — so any HMR cycle that restarts the server (the only way
/// `dx serve` rebuilds Rust changes) produces a new id. Claude's
/// `ui-validate` skill polls this to know when a rebuild has actually
/// landed.
///
/// `main.rs` calls [`init_build_id`] eagerly during boot so the id is set
/// before any request can read it; this keeps the doc accurate ("process
/// start" rather than "first health check"). Calling `build_id()` later
/// returns the same value because `OnceLock::get_or_init` is idempotent.
pub fn build_id() -> u128 {
    *BUILD_ID.get_or_init(now_millis)
}

/// Eagerly initialize [`build_id`] so the returned timestamp truly
/// represents process-start rather than first-call. Idempotent.
pub fn init_build_id() {
    let _ = BUILD_ID.get_or_init(now_millis);
}

static BUILD_ID: std::sync::OnceLock<u128> = std::sync::OnceLock::new();

fn now_millis() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

/// Absolute path of the workspace root the server was launched from —
/// captured once at process start. Surfaced via `/api/_health` so
/// `scripts/dev-server-up.sh` can tell *its own workspace's* server apart
/// from a sibling `jj` workspace's server that happens to be bound to the
/// port it's probing. Without this, port-walking would silently reuse a
/// sibling workspace's server (different code, different DB) and the
/// agent would validate against the wrong build.
///
/// `main.rs` calls [`init_repo_root`] eagerly during boot so the value is
/// set before any request can read it. `OnceLock::get_or_init` is
/// idempotent, so calling [`repo_root`] later returns the same value.
pub fn repo_root() -> &'static str {
    REPO_ROOT.get_or_init(current_dir_string)
}

/// Eagerly initialize [`repo_root`] from the process's current working
/// directory. Idempotent.
pub fn init_repo_root() {
    let _ = REPO_ROOT.get_or_init(current_dir_string);
}

static REPO_ROOT: std::sync::OnceLock<String> = std::sync::OnceLock::new();

fn current_dir_string() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|p| p.into_os_string().into_string().ok())
        .unwrap_or_default()
}

/// Attach the issue-#81 pagination hint headers to a list/search response.
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
pub(crate) mod test_support {
    use axum::{
        body::Body,
        http::{header::AUTHORIZATION, Request},
    };

    use super::*;

    /// Build a router + AppState wired against a fresh in-memory DB.
    pub(crate) async fn fixture() -> (Router, AppState, sqlx::SqlitePool) {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let state = AppState::new(pool.clone());
        let app = rest_router(state.clone());
        (app, state, pool)
    }

    /// Variant of [`fixture`] that flips the SSRF guard off (issue #275) so
    /// the wiremock-backed `put_author_photo_url` tests can drive the
    /// handler against a server bound to `127.0.0.1`. Production paths
    /// always construct `AppState::new` and therefore always block private
    /// IPs.
    pub(crate) async fn fixture_loopback_remote_image() -> (Router, AppState, sqlx::SqlitePool) {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let state = AppState::new_with_remote_image_config(
            pool.clone(),
            db::author_photos::RemoteImageConfig {
                allow_private_addresses: true,
            },
        );
        let app = rest_router(state.clone());
        (app, state, pool)
    }

    /// Convenience: GET request with a bearer auth header.
    pub(crate) fn get_with_bearer(uri: &str, token: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    /// Convenience: anonymous GET (no auth header).
    pub(crate) fn get_anon(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

    /// Seed a book row with `has_cover = 0`. Returns the inserted book id.
    pub(crate) async fn seed_book_no_cover(pool: &sqlx::SqlitePool) -> i64 {
        // Insert a minimal library row first (FK requirement).
        sqlx::query(
            "INSERT OR IGNORE INTO libraries(path, display_name) VALUES ('/test/library', 'Test')",
        )
        .execute(pool)
        .await
        .expect("insert library");
        let library_id: i64 =
            sqlx::query_scalar("SELECT id FROM libraries WHERE path = '/test/library'")
                .fetch_one(pool)
                .await
                .expect("library id");
        // Use a fixed UUID; each test gets its own in-memory pool so there is
        // no collision risk.
        sqlx::query(
            "INSERT INTO books(uuid, library_id, path, title, has_cover) VALUES (?, ?, ?, ?, 0)",
        )
        .bind("00000000-0000-0000-0000-000000000001")
        .bind(library_id)
        .bind("/test/library/no-cover.epub")
        .bind("No Cover Book")
        .execute(pool)
        .await
        .expect("insert book")
        .last_insert_rowid()
    }

    /// Seed a single book with a known title via `replace_books` and return
    /// its `id`. The book's stable UUID can be looked up afterwards via
    /// `list_books` if a test needs to assert against the overrides row
    /// directly.
    pub(crate) async fn seed_book(pool: &sqlx::SqlitePool, library: &str, title: &str) -> i64 {
        seed_book_with_uuid(pool, library, title).await.0
    }

    /// Same as `seed_book` but returns `(id, uuid)`. New tests that build
    /// uuid-keyed URLs (covers, thumbs, ebooks, overrides) use this.
    pub(crate) async fn seed_book_with_uuid(
        pool: &sqlx::SqlitePool,
        library: &str,
        title: &str,
    ) -> (i64, String) {
        db::replace_books(
            pool,
            library,
            vec![db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: format!("{title}.epub").to_lowercase(),
                    title: Some(title.to_string()),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .expect("seed_book should succeed");
        let books = db::list_books(pool, library).await.expect("list_books");
        let book = books
            .into_iter()
            .find(|b| b.title.as_deref() == Some(title))
            .expect("seeded book should be present");
        (book.id, book.unique_identifier.clone().unwrap())
    }

    /// Build a `multipart/form-data` body with a single `cover` field
    /// carrying `bytes` under the supplied content type. Returns both the
    /// `Content-Type` header value (with the boundary parameter) and the
    /// body bytes so the caller can attach them to a `Request::builder()`.
    pub(crate) fn build_cover_multipart(content_type: &str, bytes: &[u8]) -> (String, Vec<u8>) {
        let boundary = "----omnibus-test-boundary-XYZ123";
        let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 256);
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"cover\"; filename=\"cover.png\"\r\n",
        );
        body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }

    /// Minimal 1x1 transparent PNG used as a stand-in real image payload —
    /// `detect_image_format` only inspects the leading magic bytes, so this
    /// is sufficient to flow through the upload path without bundling a
    /// fixture file.
    pub(crate) const TINY_PNG: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0A, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    /// Process-global `OMNIBUS_COVERS_DIR` lock — the cover-upload tests
    /// each install their own scratch dir via `set_var`, so they must
    /// serialize with each other (and with anything else in this crate
    /// that swaps the same env var). Mirrors the `COVERS_ENV_LOCK` in
    /// `db::queries::tests`.
    pub(crate) static COVER_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard that points `OMNIBUS_COVERS_DIR` at a fresh scratch dir
    /// for the duration of a single test and restores the previous value
    /// (or removes the var) on drop. Holds the `COVER_DIR_ENV_LOCK` so
    /// parallel cover tests serialize their env-var writes.
    pub(crate) struct CoversDirGuard {
        path: std::path::PathBuf,
        prev: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl CoversDirGuard {
        pub(crate) fn new(tag: &str) -> Self {
            let guard = COVER_DIR_ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let pid = std::process::id();
            let nanos = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path =
                std::env::temp_dir().join(format!("omnibus_rest_covers_{tag}_{pid}_{nanos}"));
            let _ = std::fs::remove_dir_all(&path);
            std::fs::create_dir_all(&path).expect("create covers scratch dir");
            let prev = std::env::var("OMNIBUS_COVERS_DIR").ok();
            std::env::set_var("OMNIBUS_COVERS_DIR", &path);
            Self {
                path,
                prev,
                _guard: guard,
            }
        }
    }

    impl Drop for CoversDirGuard {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
            match self.prev.take() {
                Some(v) => std::env::set_var("OMNIBUS_COVERS_DIR", v),
                None => std::env::remove_var("OMNIBUS_COVERS_DIR"),
            }
        }
    }

    pub(crate) async fn seed_author(pool: &sqlx::SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar::<_, i64>("INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id")
            .bind(name)
            .bind(name)
            .fetch_one(pool)
            .await
            .expect("seed author")
    }

    /// Multipart body with one `photo` field, matching `build_cover_multipart`
    /// but using the field name the author-photo handler expects.
    pub(crate) fn build_photo_multipart(content_type: &str, bytes: &[u8]) -> (String, Vec<u8>) {
        let boundary = "----omnibus-test-photo-boundary";
        let mut body: Vec<u8> = Vec::with_capacity(bytes.len() + 256);
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        body.extend_from_slice(
            b"Content-Disposition: form-data; name=\"photo\"; filename=\"photo.png\"\r\n",
        );
        body.extend_from_slice(format!("Content-Type: {content_type}\r\n\r\n").as_bytes());
        body.extend_from_slice(bytes);
        body.extend_from_slice(format!("\r\n--{boundary}--\r\n").as_bytes());
        (format!("multipart/form-data; boundary={boundary}"), body)
    }

    /// JSON PUT helper for the `/api/authors/:id/photo/url` route.
    pub(crate) fn put_photo_url(uri: &str, token: &str, url: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .method("PUT")
            .header("content-type", "application/json")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(serde_json::json!({ "url": url }).to_string()))
            .unwrap()
    }
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{to_bytes, Body},
        http::{header::AUTHORIZATION, Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::*;
    use crate::auth::test_support as auth_test_support;
    use crate::backend::test_support::*;

    // -------------------------------------------------------------------
    // /api/_health — unauthenticated liveness + fingerprint.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_health_returns_200_unauth_with_app_and_build_id() {
        let (app, _state, _pool) = fixture().await;
        let res = app
            .oneshot(get_anon("/api/_health"))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::OK);

        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body: serde_json::Value = serde_json::from_slice(&bytes).expect("JSON body");
        assert_eq!(body["app"], "omnibus");
        assert_eq!(body["status"], "ok");
        let build_id = body["build_id"]
            .as_str()
            .expect("build_id should be string");
        assert!(
            build_id.chars().all(|c| c.is_ascii_digit()),
            "build_id should be all digits, got {build_id:?}"
        );
        // `repo_root` is the workspace-identity field scripts/dev-server-up.sh
        // parses to distinguish this workspace's server from a sibling
        // jj workspace's server bound to the same port. Must be a string.
        assert!(
            body["repo_root"].is_string(),
            "repo_root must be a string, got {:?}",
            body["repo_root"]
        );
    }

    // -------------------------------------------------------------------
    // Global request guards — protects against slow clients / oversized
    // bodies holding a tokio worker indefinitely. See #85.
    // -------------------------------------------------------------------

    /// POSTing a JSON body well over the 1 MiB global cap should be
    /// rejected with 413 PAYLOAD_TOO_LARGE before the handler ever sees
    /// it. We pad the JSON with a long throwaway string so the body
    /// exceeds the cap while staying syntactically valid.
    #[tokio::test]
    async fn api_post_settings_rejects_body_over_1mb_with_413() {
        let (app, _state, pool) = fixture().await;
        let admin = auth_test_support::create_admin(&pool, "admin").await;
        let token = auth_test_support::bearer_token(&pool, admin.id).await;

        // 2 MiB of filler — comfortably over the 1 MiB cap, but small
        // enough that allocating it in-test is cheap.
        let filler = "a".repeat(2 * 1024 * 1024);
        let body = serde_json::json!({
            "ebook_library_path": filler,
            "audiobook_library_path": "/books/audio"
        });
        let bytes = body.to_string();
        assert!(
            bytes.len() > 1024 * 1024,
            "test body must exceed the 1 MiB cap; got {} bytes",
            bytes.len()
        );

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/settings")
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(bytes))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[tokio::test]
    async fn api_upload_endpoints_share_per_ip_budget_and_exclude_reads() {
        // #168: the three binary-upload routes (cover POST, author-photo PUT,
        // photo-URL PUT) share ONE per-IP fixed-window limiter
        // (UPLOAD_RATE_LIMIT_MAX per UPLOAD_RATE_LIMIT_WINDOW) in
        // `upload_router`; the GET/DELETE photo routes live outside it and
        // carry no limiter. The limiter runs before the handler, so a
        // handler's own status doesn't matter — we only assert 429 vs not.
        // oneshot requests carry no ConnectInfo<SocketAddr>, so they all
        // share the limiter's 0.0.0.0 fallback bucket.
        let (app, _state, pool) = fixture().await;
        let admin = auth_test_support::create_admin(&pool, "admin").await;
        let token = auth_test_support::bearer_token(&pool, admin.id).await;

        let cover_post = || {
            Request::builder()
                .uri("/api/ebooks/1/cover")
                .method("POST")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from("x"))
                .unwrap()
        };
        let photo_put = || {
            Request::builder()
                .uri("/api/authors/1/photo")
                .method("PUT")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from("x"))
                .unwrap()
        };
        let photo_url_put = || {
            Request::builder()
                .uri("/api/authors/1/photo/url")
                .method("PUT")
                .header("content-type", "application/json")
                .header(AUTHORIZATION, format!("Bearer {token}"))
                .body(Body::from(r#"{"url":"http://127.0.0.1:1/x.jpg"}"#))
                .unwrap()
        };

        // Spend the shared budget. Each is within budget, so none should 429.
        for i in 0..UPLOAD_RATE_LIMIT_MAX {
            let res = app
                .clone()
                .oneshot(photo_url_put())
                .await
                .expect("request should succeed");
            assert_ne!(
                res.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "request #{} should be within the shared upload budget",
                i + 1
            );
        }

        // Budget is now spent: every upload route trips the shared limiter,
        // proving the cap covers all three (not just photo-url).
        for (label, req) in [
            ("POST /api/ebooks/{id}/cover", cover_post()),
            ("PUT /api/authors/{id}/photo", photo_put()),
            ("PUT /api/authors/{id}/photo/url", photo_url_put()),
        ] {
            let res = app
                .clone()
                .oneshot(req)
                .await
                .expect("request should succeed");
            assert_eq!(
                res.status(),
                StatusCode::TOO_MANY_REQUESTS,
                "{label} must return 429 once the shared upload budget is spent",
            );
        }

        // The read/non-upload photo routes are outside upload_router, so they
        // stay unthrottled even after the upload budget is exhausted.
        let get = app
            .clone()
            .oneshot(get_with_bearer("/api/authors/1/photo", &token))
            .await
            .expect("request should succeed");
        assert_ne!(
            get.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "GET author photo must not be rate-limited by the upload limiter",
        );
        let del = app
            .oneshot(
                Request::builder()
                    .uri("/api/authors/1/photo")
                    .method("DELETE")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_ne!(
            del.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "DELETE author photo must not be rate-limited by the upload limiter",
        );
    }
}
