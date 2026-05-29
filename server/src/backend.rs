//! Hand-written `/api/*` REST routes for the mobile client.
//!
//! Web uses Dioxus server functions (see `omnibus_frontend::rpc`), mounted
//! automatically by `dioxus::server::router(App)`. These REST routes are
//! merged alongside them in `main.rs` so mobile's existing `reqwest` paths
//! keep working unchanged.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::header,
    response::{IntoResponse, Response},
    routing::{get, post, put},
    Extension, Json, Router,
};
use omnibus_db::{
    self as db, scanner,
    worker::{Task, TaskOutcome, Worker, WorkerConfig},
};
use omnibus_shared::{detect_image_format, MetadataOverrides, Settings, ValueResponse};
use serde::Deserialize;
use sqlx::SqlitePool;

use crate::auth::{AdminUser, AuthUser};
use crate::rate_limit::{rate_limit_by_ip, RateLimiter};

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
}

impl AppState {
    pub fn new(pool: SqlitePool) -> Self {
        let worker = Worker::new(pool.clone(), WorkerConfig::default());
        Self { pool, worker }
    }

    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub fn worker(&self) -> &Arc<Worker> {
        &self.worker
    }
}

pub fn rest_router(state: AppState) -> Router {
    // Standalone/test entrypoint: a fresh, dedicated search limiter, so each
    // `rest_router` call (and thus each integration test) gets its own bucket
    // map. The live server instead calls `rest_router_with_search_limiter` so
    // the REST and RPC search families share one per-IP budget (#249).
    rest_router_with_search_limiter(
        state,
        std::sync::Arc::new(RateLimiter::with_policy(
            SEARCH_RATE_LIMIT_WINDOW,
            SEARCH_RATE_LIMIT_MAX,
        )),
    )
}

/// Build the `/api/*` REST router, sharing `search_limiter` with the caller.
/// `main.rs` passes the *same* `Arc<RateLimiter>` here and to the RPC
/// `rate_limit_paths` layer so `/api/search/*` (REST) and `/api/rpc/search-*`
/// (RPC) draw down a single per-IP budget instead of one each (#249).
pub fn rest_router_with_search_limiter(
    state: AppState,
    search_limiter: std::sync::Arc<RateLimiter>,
) -> Router {
    let pool = state.pool().clone();
    Router::new()
        .route("/api/_health", get(get_health))
        .route("/api/value", get(get_value))
        .route("/api/value/increment", post(increment_value))
        .route("/api/settings", get(get_settings))
        .route("/api/settings", post(post_settings))
        .route("/api/reindex", post(post_reindex))
        .route("/api/library", get(get_library))
        .route("/api/ebooks", get(get_ebooks))
        .route("/api/ebooks/{uuid}", get(get_ebook_by_uuid))
        .route(
            "/api/ebooks/{uuid}/overrides",
            post(post_ebook_overrides).delete(delete_ebook_overrides),
        )
        // GET/DELETE for author photos carry no upload body (DELETE mutates,
        // but cheaply — it clears photo state, it doesn't ingest one), so
        // they stay outside the rate-limited `upload_router`. Only the binary
        // uploads (cover POST, photo PUT, photo-url PUT) carry the per-IP
        // frequency cap — see `upload_router` (#168).
        .route(
            "/api/authors/{id}/photo",
            get(get_author_photo).delete(delete_author_photo),
        )
        .merge(upload_router())
        .merge(search_router(search_limiter))
        .route("/api/covers/{uuid}", get(get_cover))
        .route("/api/thumbs/{uuid}/{size}", get(get_thumb))
        .route("/api/authors", get(get_authors))
        .route("/api/authors/{id}/photo/scan", post(post_author_photo_scan))
        .route("/api/authors/{id}", get(get_author_by_id))
        .route("/api/series", get(get_series))
        .route("/api/series/{id}", get(get_series_by_id))
        .route("/api/tags", get(get_tags))
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
/// The `limiter` is passed in (not built here) so the live server can share
/// the same `Arc` with the RPC `/api/rpc/search-*` layer for a single per-IP
/// budget across both search families (#249); `rest_router` supplies a fresh
/// dedicated one for standalone/test use.
fn search_router(limiter: std::sync::Arc<RateLimiter>) -> Router<AppState> {
    Router::new()
        .route("/api/search", get(get_search))
        .route("/api/search/palette", get(get_search_palette))
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
        .route("/api/ebooks/{uuid}/cover", post(post_ebook_cover))
        .route("/api/authors/{id}/photo", put(put_author_photo))
        .route("/api/authors/{id}/photo/url", put(put_author_photo_url))
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

/// Unauthenticated liveness + fingerprint endpoint. The `app` field lets
/// `scripts/dev-server-up.sh` distinguish an omnibus instance from some
/// other process that happens to bind the same port. Whitelisted in
/// `auth::gate::require_auth` so it remains reachable without a session.
async fn get_health() -> Response {
    Json(serde_json::json!({
        "app": "omnibus",
        "status": "ok",
        "build_id": build_id().to_string(),
    }))
    .into_response()
}

async fn get_value(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::get_value(&state.pool).await {
        Ok(value) => Json(ValueResponse { value }).into_response(),
        Err(error) => internal("read value", error),
    }
}

async fn increment_value(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::increment_value(&state.pool).await {
        Ok(value) => Json(ValueResponse { value }).into_response(),
        Err(error) => internal("increment value", error),
    }
}

async fn get_settings(_admin: AdminUser, State(state): State<AppState>) -> Response {
    match db::get_settings(&state.pool).await {
        Ok(settings) => Json(settings).into_response(),
        Err(error) => internal("read settings", error),
    }
}

async fn post_settings(
    _admin: AdminUser,
    State(state): State<AppState>,
    Json(settings): Json<Settings>,
) -> Response {
    match db::set_settings(&state.pool, &settings).await {
        Ok(()) => match db::get_settings(&state.pool).await {
            Ok(updated) => {
                // Library path may have changed (and even when it hasn't,
                // the user has signalled they want to pick up on-disk
                // changes). Hand the reindex to the shared Worker so the
                // per-path mutex serializes overlapping saves and the
                // scan_concurrency cap stays honored.
                let task_id = updated
                    .ebook_library_path
                    .clone()
                    .map(|library_path| state.worker.post(Task::Scan { library_path }));

                let mut response = Json(updated).into_response();
                #[cfg(debug_assertions)]
                if let Some(id) = task_id {
                    if let Ok(value) = id.to_string().parse::<axum::http::HeaderValue>() {
                        response
                            .headers_mut()
                            .insert("X-Omnibus-Worker-Task-Id", value);
                    }
                }
                #[cfg(not(debug_assertions))]
                let _ = task_id;
                response
            }
            Err(error) => internal("read updated settings", error),
        },
        Err(error) => internal("save settings", error),
    }
}

/// Admin-only synchronous reindex: 200 on success, 409 when no library
/// path is configured, 500 on worker failure. Regression target for #112.
async fn post_reindex(_admin: AdminUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    let Some(library_path) = settings.ebook_library_path else {
        return (
            axum::http::StatusCode::CONFLICT,
            "no ebook library path configured",
        )
            .into_response();
    };
    let task_id = state.worker.post(Task::Scan { library_path });
    match state.worker.await_completion(task_id).await {
        TaskOutcome::Ok => axum::http::StatusCode::OK.into_response(),
        TaskOutcome::Err(e) => internal("reindex", e),
    }
}

async fn get_ebooks(_user: AuthUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    match db::library_from_db_with_total(&state.pool, settings.ebook_library_path.as_deref()).await
    {
        Ok((library, total)) => with_pagination_headers(Json(library).into_response(), total),
        Err(error) => internal("read books", error),
    }
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

async fn get_ebook_by_uuid(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    match db::get_book_by_uuid(&state.pool, &uuid).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(error) => internal("read book", error),
    }
}

#[derive(Deserialize)]
struct SearchQuery {
    q: String,
}

async fn get_search(
    _user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    let Some(path) = settings.ebook_library_path else {
        // Match the `/api/ebooks` contract: even an empty result attaches
        // `X-Total-Count: 0` so clients can rely on the header always
        // being present.
        return with_pagination_headers(
            Json(omnibus_shared::EbookLibrary::default()).into_response(),
            0,
        );
    };
    let books = match db::search_books(&state.pool, &path, &params.q).await {
        Ok(b) => b,
        Err(error) => return internal("search books", error),
    };
    // Issue #81: return the *full* hit count alongside the (capped) vec
    // so clients can detect truncation via the `X-Total-Count` /
    // `X-Total-Cap` headers without changing the JSON body shape.
    let total = match db::count_search_books(&state.pool, &path, &params.q).await {
        Ok(t) => t,
        Err(error) => return internal("count search books", error),
    };
    let body = Json(omnibus_shared::EbookLibrary {
        path: Some(path),
        books,
        error: None,
    })
    .into_response();
    with_pagination_headers(body, total)
}

async fn get_search_palette(
    _user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    let Some(path) = settings.ebook_library_path else {
        return Json(omnibus_shared::PaletteResults::default()).into_response();
    };
    match db::search_palette(&state.pool, &path, &params.q).await {
        Ok(results) => Json(results).into_response(),
        Err(error) => internal("search palette", error),
    }
}

async fn get_cover(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    // Resolve uuid → id so the existing id-keyed `db::get_cover` (which
    // reads cover bytes from `<covers_dir>/<uuid>.<ext>` by way of the
    // books row) stays unchanged. The route surface is uuid-keyed so
    // bookmarked URLs survive reindexes; the storage layer keeps using
    // the autoincrement id internally for join performance.
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    match db::get_cover(&state.pool, id).await {
        Ok(Some((mime, bytes))) => (
            [
                (header::CONTENT_TYPE, mime.as_str()),
                // Covers are static per-book (new id on reindex). Cached on
                // the client only — `private` + `Vary: Cookie` keep a shared
                // proxy from serving one user's covers to an unauthenticated
                // request on the same URL now that the endpoint is gated.
                (header::CACHE_CONTROL, "private, max-age=86400"),
                (header::VARY, "Cookie"),
                // Prevent browsers from MIME-sniffing a cover into an
                // executable type (e.g. an SVG disguised as JPEG).
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        Ok(None) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(error) => internal("read cover", error),
    }
}

async fn get_thumb(
    _user: AuthUser,
    State(state): State<AppState>,
    Path((uuid, size_str)): Path<(String, String)>,
) -> Response {
    let size: db::ThumbSize = match size_str.parse() {
        Ok(s) => s,
        Err(_) => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "invalid size; use sm, md, or lg",
            )
                .into_response();
        }
    };
    // uuid → id translation: see the comment in `get_cover` for why we
    // resolve at the edge rather than rewriting the thumbnail pipeline
    // to be uuid-keyed end-to-end.
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };

    let last_modified_epoch = match db::get_last_modified_epoch(&state.pool, id).await {
        Ok(Some(ts)) => ts,
        Ok(None) => return axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(e) => return internal("read last_modified_epoch", e),
    };

    // Cache hit: thumb exists and is fresh. Use async I/O here so a hot
    // `srcset` grid doesn't pin tokio worker threads on the synchronous read.
    let thumb_path = db::thumb_path_for(id, size);
    if !db::thumbs::is_stale_async(id, size, last_modified_epoch).await {
        if let Ok(bytes) = tokio::fs::read(&thumb_path).await {
            return (
                [
                    (header::CONTENT_TYPE, "image/webp"),
                    (header::CACHE_CONTROL, "private, max-age=86400"),
                    (header::VARY, "Cookie"),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                ],
                bytes,
            )
                .into_response();
        }
    }

    // Cache miss or stale: fetch the original cover first so we only queue
    // generation when there's actually something to thumbnail. Queuing for
    // a coverless book just produces a guaranteed `no cover for book …`
    // worker error on every request, polluting the log.
    match db::get_cover(&state.pool, id).await {
        Ok(Some((mime, bytes))) => {
            state.worker.post(db::worker::Task::GenerateThumbs {
                book_id: id,
                last_modified_epoch,
            });
            (
                [
                    (header::CONTENT_TYPE, mime.as_str()),
                    // Short TTL: browser will re-fetch after ~5 s when the WebP is ready.
                    (header::CACHE_CONTROL, "private, max-age=5"),
                    (header::VARY, "Cookie"),
                    (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
                ],
                bytes,
            )
                .into_response()
        }
        Ok(None) => axum::http::StatusCode::ACCEPTED.into_response(),
        Err(e) => internal("cover fetch for thumb", e),
    }
}

async fn get_author_by_id(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::get_author(&state.pool, id).await {
        Ok(Some(author)) => {
            // F1.11: queue a background resolution when the author has no
            // `author_photos` row yet so first-time visits trigger Open
            // Library resolution. A subsequent visit picks up the resolved
            // photo (or the `letter` negative-cache marker).
            if !author.has_photo {
                match db::author_photo_status(&state.pool, id).await {
                    Ok(None) => {
                        state
                            .worker
                            .post(Task::ResolveAuthorPhoto { author_id: id });
                    }
                    Ok(Some(_)) => {}
                    Err(e) => {
                        // Don't fail the read — just skip the queue and log.
                        tracing::warn!(
                            author_id = id,
                            error = %e,
                            "author_photo_status check failed; skipping autoresolution"
                        );
                    }
                }
            }
            Json(author).into_response()
        }
        Ok(None) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(error) => internal("read author", error),
    }
}

/// F1.12 — `/authors` index. Returns every author in the configured
/// library with a book count and optional accent. Empty list when no
/// library is configured.
async fn get_authors(_user: AuthUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(e) => return internal("read settings", e),
    };
    let Some(path) = settings.ebook_library_path else {
        return Json(Vec::<omnibus_shared::AuthorSummary>::new()).into_response();
    };
    match db::list_authors(&state.pool, &path).await {
        Ok(authors) => Json(authors).into_response(),
        Err(e) => internal("list authors", e),
    }
}

/// F1.12 — `/series` index. Returns every series in the configured
/// library with a book count, primary author, and optional accent.
async fn get_series(_user: AuthUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(e) => return internal("read settings", e),
    };
    let Some(path) = settings.ebook_library_path else {
        return Json(Vec::<omnibus_shared::SeriesSummary>::new()).into_response();
    };
    match db::list_series(&state.pool, &path).await {
        Ok(series) => Json(series).into_response(),
        Err(e) => internal("list series", e),
    }
}

async fn get_series_by_id(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::get_series(&state.pool, id).await {
        Ok(Some(series)) => Json(series).into_response(),
        Ok(None) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(error) => internal("read series", error),
    }
}

async fn get_tags(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::get_tag_cloud(&state.pool).await {
        Ok(tags) => Json(tags).into_response(),
        Err(error) => internal("read tags", error),
    }
}

async fn get_library(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::get_settings(&state.pool).await {
        Ok(settings) => {
            let contents = scanner::scan_libraries(
                settings.ebook_library_path.as_deref(),
                settings.audiobook_library_path.as_deref(),
            );
            Json(contents).into_response()
        }
        Err(error) => internal("read settings", error),
    }
}

// ---------------------------------------------------------------------------
// F5.1 Metadata overrides (REST — mobile client).
// ---------------------------------------------------------------------------

/// Save metadata overrides for a book. Requires `can_edit` or admin.
async fn post_ebook_overrides(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    Json(overrides): Json<MetadataOverrides>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }
    if let Err(msg) = overrides.validate() {
        return (axum::http::StatusCode::BAD_REQUEST, msg).into_response();
    }
    // Resolve uuid → id so the thumbnail/cover invalidate calls stay
    // id-keyed. Returns 404 for unknown uuids — same behavior the old
    // `get_book_uuid(id)` -> 404 had for unknown ids.
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    // Merge incoming overrides with any existing ones so a second edit that
    // only touches field B doesn't wipe a prior override on field A. The
    // read-merge-write is serialized inside a single BEGIN IMMEDIATE
    // transaction in the db layer so two concurrent edits to the same book
    // can't interleave and drop each other's changes (#166).
    if let Err(e) = db::merge_metadata_overrides(&state.pool, &uuid, &overrides, user.id).await {
        return internal("merge_metadata_overrides", e);
    }
    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => internal("get_book", e),
    }
}

/// Delete metadata overrides for a book, reverting to scanned values.
async fn delete_ebook_overrides(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };
    if let Err(e) = db::delete_metadata_overrides(&state.pool, &uuid).await {
        return internal("delete_metadata_overrides", e);
    }
    // `delete_override_cover` + `invalidate_thumbs` are sync `std::fs`
    // operations; run them on the blocking pool so the axum runtime stays
    // responsive under load (#106).
    let uuid_for_blocking = uuid.clone();
    if let Err(e) = tokio::task::spawn_blocking(move || {
        db::delete_override_cover(&uuid_for_blocking);
        db::thumbs::invalidate_thumbs(id);
    })
    .await
    {
        return internal("spawn_blocking(delete_override_cover)", e);
    }
    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => internal("get_book", e),
    }
}

/// Upload a replacement cover image for a book. Multipart form with a single
/// `cover` field containing the image bytes.
async fn post_ebook_cover(
    user: AuthUser,
    State(state): State<AppState>,
    Path(uuid): Path<String>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    if !user.is_admin && !user.can_edit {
        return (
            axum::http::StatusCode::FORBIDDEN,
            "edit permission required",
        )
            .into_response();
    }

    // uuid → id for the thumb-invalidate + `get_book` calls that still
    // use the internal autoincrement key. Returns 404 for unknown uuids,
    // matching the prior `get_book_uuid(id)` → 404 behavior.
    let id = match db::resolve_book_id_by_uuid(&state.pool, &uuid).await {
        Ok(Some(id)) => id,
        Ok(None) => return (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => return internal("resolve_book_id_by_uuid", e),
    };

    // Extract the cover field from the multipart body.
    let (mime, bytes) = loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or("").to_string();
                if name != "cover" {
                    continue;
                }
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                if !content_type.starts_with("image/") {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "cover must be an image",
                    )
                        .into_response();
                }
                // Reject SVG — contains executable content and can XSS when
                // opened directly in a browser tab.
                if content_type.contains("svg") {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "SVG covers are not accepted",
                    )
                        .into_response();
                }
                match field.bytes().await {
                    Ok(b) => {
                        if b.len() > 10 * 1024 * 1024 {
                            return (
                                axum::http::StatusCode::BAD_REQUEST,
                                "cover must be under 10 MB",
                            )
                                .into_response();
                        }
                        // Validate magic bytes — don't trust Content-Type alone.
                        // Bind the detected MIME directly: a `None` here means the
                        // bytes carry no recognisable image header, so surface a
                        // 415 rather than `.unwrap()`-panicking the task (#210).
                        match detect_image_format(&b) {
                            // Use the detected MIME so the stored extension matches
                            // actual content, not the (untrusted) client header.
                            Some(mime) => break (mime, b),
                            None => {
                                return (
                                    axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                                    "Could not detect image format",
                                )
                                    .into_response();
                            }
                        }
                    }
                    Err(e) => return internal("read cover field", e),
                }
            }
            Ok(None) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    "missing 'cover' field in multipart body",
                )
                    .into_response()
            }
            Err(e) => return internal("parse multipart", e),
        }
    };

    // Write the override cover to disk. `write_override_cover` is a sync
    // `std::fs` call — run it on the blocking pool so the axum runtime stays
    // responsive while we hit disk (#106). `uuid` is needed again below for
    // the overrides table update, so it's the only value we clone.
    let uuid_for_write = uuid.clone();
    let write_result = tokio::task::spawn_blocking(move || {
        db::write_override_cover(&uuid_for_write, &mime, &bytes)
    })
    .await;
    match write_result {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return internal("write_override_cover", e),
        Err(e) => return internal("spawn_blocking(write_override_cover)", e),
    }

    // Mark the overrides table with has_cover_override = 1. Preserve existing
    // field overrides if any.
    let existing_overrides = match db::get_metadata_overrides(&state.pool, &uuid).await {
        Ok(Some((ov, _))) => ov,
        Ok(None) => MetadataOverrides::default(),
        Err(e) => return internal("get_metadata_overrides", e),
    };
    if let Err(e) =
        db::upsert_metadata_overrides(&state.pool, &uuid, &existing_overrides, true, user.id).await
    {
        return internal("upsert_metadata_overrides", e);
    }

    // Invalidate thumb cache so next request regenerates from new cover.
    // Also sync `std::fs` — run on the blocking pool (#106).
    if let Err(e) = tokio::task::spawn_blocking(move || db::thumbs::invalidate_thumbs(id)).await {
        return internal("spawn_blocking(invalidate_thumbs)", e);
    }

    match db::get_book(&state.pool, id).await {
        Ok(Some(book)) => Json(book).into_response(),
        Ok(None) => (axum::http::StatusCode::NOT_FOUND, "book not found").into_response(),
        Err(e) => internal("get_book", e),
    }
}

// ---------------------------------------------------------------------------
// F1.11 Author profile photos.
// ---------------------------------------------------------------------------

/// Serve a cached author profile photo. Returns 404 when no photo is cached
/// (including `'letter'` negative-cache markers) — the frontend keeps the
/// letter avatar in that case. On a miss, enqueues a background resolution
/// task so a subsequent page view can render the resolved photo.
async fn get_author_photo(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::get_author_photo(&state.pool, id).await {
        Ok(Some((mime, bytes))) => (
            [
                (header::CONTENT_TYPE, mime.as_str()),
                // Match `/api/covers/:id` cache semantics.
                (header::CACHE_CONTROL, "private, max-age=86400"),
                (header::VARY, "Cookie"),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bytes,
        )
            .into_response(),
        Ok(None) => {
            // Only queue resolution if no row exists yet — a `letter` marker
            // is a sticky miss until an admin DELETEs it. `author_photo_status`
            // returns the row regardless of source.
            if let Ok(None) = db::author_photo_status(&state.pool, id).await {
                state
                    .worker
                    .post(Task::ResolveAuthorPhoto { author_id: id });
            }
            axum::http::StatusCode::NOT_FOUND.into_response()
        }
        Err(error) => internal("read author photo", error),
    }
}

/// Admin upload of a manual author profile photo. Multipart with one
/// `photo` field. Mirrors the `/api/ebooks/{id}/cover` validation
/// pipeline: 10 MiB cap, magic-byte sniff, SVG rejection.
async fn put_author_photo(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    mut multipart: axum::extract::Multipart,
) -> Response {
    // Confirm the author exists before reading multipart so a malformed
    // upload to a missing id fails fast with 404 (not 400).
    let author_exists: bool =
        match sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM authors WHERE id = ?)")
            .bind(id)
            .fetch_one(&state.pool)
            .await
        {
            Ok(v) => v != 0,
            Err(e) => return internal("author exists check", e),
        };
    if !author_exists {
        return (axum::http::StatusCode::NOT_FOUND, "author not found").into_response();
    }

    let (mime, bytes) = loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or("").to_string();
                if name != "photo" {
                    continue;
                }
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                if !content_type.starts_with("image/") {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "photo must be an image",
                    )
                        .into_response();
                }
                if content_type.contains("svg") {
                    return (
                        axum::http::StatusCode::BAD_REQUEST,
                        "SVG photos are not accepted",
                    )
                        .into_response();
                }
                match field.bytes().await {
                    Ok(b) => {
                        if b.len() > 10 * 1024 * 1024 {
                            return (
                                axum::http::StatusCode::BAD_REQUEST,
                                "photo must be under 10 MB",
                            )
                                .into_response();
                        }
                        // Bind the detected MIME directly: a `None` here means the
                        // bytes carry no recognisable image header, so surface a
                        // 415 rather than `.unwrap()`-panicking the task (#210).
                        match detect_image_format(&b) {
                            Some(mime) => break (mime, b),
                            None => {
                                return (
                                    axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                                    "Could not detect image format",
                                )
                                    .into_response();
                            }
                        }
                    }
                    Err(e) => return internal("read photo field", e),
                }
            }
            Ok(None) => {
                return (
                    axum::http::StatusCode::BAD_REQUEST,
                    "missing 'photo' field in multipart body",
                )
                    .into_response()
            }
            Err(e) => return internal("parse multipart", e),
        }
    };

    if let Err(e) = db::upsert_author_photo(
        &state.pool,
        id,
        db::AuthorPhotoSource::Manual,
        None,
        Some(&mime),
        Some(&bytes),
    )
    .await
    {
        return internal("upsert_author_photo", e);
    }
    axum::http::StatusCode::NO_CONTENT.into_response()
}

/// Admin: drop the cached photo so the next page view re-queues resolution.
async fn delete_author_photo(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::delete_author_photo(&state.pool, id).await {
        Ok(()) => axum::http::StatusCode::NO_CONTENT.into_response(),
        Err(e) => internal("delete_author_photo", e),
    }
}

/// Admin: synchronously run the Open Library cascade for an author. Clears
/// any sticky `letter` negative-cache marker so the resolver re-queries Open
/// Library, then awaits the resolver inline and returns
/// `{ "resolved": bool }`. `resolved=false` means Open Library had nothing
/// (a `letter` marker is now in place to skip future autoresolution).
///
/// Manual uploads are treated as overrides: a `source = 'manual'` row is
/// preserved (the F1.11 roadmap explicitly calls this out — "skips if a
/// manual override exists"). Scan returns `resolved=true` in that case
/// without touching the row, so admins can't accidentally wipe a manual
/// upload by clicking the button.
async fn post_author_photo_scan(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    // Verify the author exists first so a typo on the id gets a 404 instead
    // of a successful no-op scan.
    let author_exists: bool =
        match sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM authors WHERE id = ?)")
            .bind(id)
            .fetch_one(&state.pool)
            .await
        {
            Ok(v) => v != 0,
            Err(e) => return internal("author exists check", e),
        };
    if !author_exists {
        return (axum::http::StatusCode::NOT_FOUND, "author not found").into_response();
    }
    // Manual uploads win — don't delete or overwrite. Treat scan as a no-op
    // and report resolved=true so the UI keeps the existing photo.
    match db::author_photo_status(&state.pool, id).await {
        Ok(Some((db::AuthorPhotoSource::Manual, _))) => {
            return Json(omnibus_shared::AuthorPhotoScanResult { resolved: true }).into_response();
        }
        Ok(_) => {}
        Err(e) => return internal("author_photo_status (pre-scan)", e),
    }
    if let Err(e) = db::delete_author_photo(&state.pool, id).await {
        return internal("delete_author_photo (pre-scan)", e);
    }
    if let Err(e) = db::author_photos::resolve(&state.pool, id).await {
        return internal("author_photos::resolve", e);
    }
    let resolved = match db::get_author_photo(&state.pool, id).await {
        Ok(opt) => opt.is_some(),
        Err(e) => return internal("get_author_photo (post-scan)", e),
    };
    Json(omnibus_shared::AuthorPhotoScanResult { resolved }).into_response()
}

/// JSON body for [`put_author_photo_url`]. Kept inline because the shape is
/// trivial and not shared with any other call site (the RPC server function
/// passes the URL as a positional arg).
#[derive(Debug, Deserialize)]
struct AuthorPhotoUrlBody {
    url: String,
}

/// Admin: persist an author photo by URL. Server-side fetches the URL,
/// validates content-type/size/magic-bytes, and stores it as a `manual`
/// row — the same source as a multipart upload, so it wins over Open
/// Library resolution and survives a "Scan for picture" click.
async fn put_author_photo_url(
    _admin: AdminUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
    Json(body): Json<AuthorPhotoUrlBody>,
) -> Response {
    let author_exists: bool =
        match sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM authors WHERE id = ?)")
            .bind(id)
            .fetch_one(&state.pool)
            .await
        {
            Ok(v) => v != 0,
            Err(e) => return internal("author exists check", e),
        };
    if !author_exists {
        return (axum::http::StatusCode::NOT_FOUND, "author not found").into_response();
    }

    let url = body.url.trim();
    if url.is_empty() {
        return (axum::http::StatusCode::BAD_REQUEST, "url is required").into_response();
    }

    let (advertised_mime, bytes) = match db::author_photos::fetch_remote_image(url).await {
        Ok(pair) => pair,
        Err(db::author_photos::FetchRemoteImageError::Http(e)) => {
            return internal("fetch remote image", e);
        }
        Err(e) => {
            // All other variants are validation errors (bad scheme, non-image
            // content-type, too-large, …) — map to 400 with the user-facing
            // message from `#[error]`.
            return (axum::http::StatusCode::BAD_REQUEST, e.to_string()).into_response();
        }
    };

    // Magic-byte sniff — don't trust the remote Content-Type header.
    let mime = match detect_image_format(&bytes) {
        Some(m) => m,
        None => {
            tracing::warn!(
                advertised_mime,
                "remote URL returned image content-type but bytes are not a recognized image"
            );
            return (
                axum::http::StatusCode::BAD_REQUEST,
                "file at URL does not appear to be a valid image",
            )
                .into_response();
        }
    };

    if let Err(e) = db::upsert_author_photo(
        &state.pool,
        id,
        db::AuthorPhotoSource::Manual,
        Some(url),
        Some(&mime),
        Some(&bytes),
    )
    .await
    {
        return internal("upsert_author_photo (url)", e);
    }
    axum::http::StatusCode::NO_CONTENT.into_response()
}

#[cfg(test)]
mod tests {
    use axum::{
        body::{to_bytes, Body},
        http::{header::AUTHORIZATION, Request, StatusCode},
    };
    use tower::ServiceExt;

    use super::*;
    use crate::auth::test_support;

    /// Build a router + AppState wired against a fresh in-memory DB.
    async fn fixture() -> (Router, AppState, sqlx::SqlitePool) {
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let state = AppState::new(pool.clone());
        let app = rest_router(state.clone());
        (app, state, pool)
    }

    /// Convenience: GET request with a bearer auth header.
    fn get_with_bearer(uri: &str, token: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::empty())
            .unwrap()
    }

    /// Convenience: anonymous GET (no auth header).
    fn get_anon(uri: &str) -> Request<Body> {
        Request::builder().uri(uri).body(Body::empty()).unwrap()
    }

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
    }

    // -------------------------------------------------------------------
    // Happy paths — every protected route bootstraps the appropriate user.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_reads_and_increments_value() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let response = app
            .clone()
            .oneshot(get_with_bearer("/api/value", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: ValueResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload.value, 0);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/value/increment")
                    .method("POST")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let payload: ValueResponse = serde_json::from_slice(&body).unwrap();
        assert_eq!(payload.value, 1);
    }

    #[tokio::test]
    async fn api_get_settings_returns_null_defaults() {
        let (app, _state, pool) = fixture().await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let response = app
            .oneshot(get_with_bearer("/api/settings", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let settings: Settings = serde_json::from_slice(&body).unwrap();
        assert_eq!(settings.ebook_library_path, None);
        assert_eq!(settings.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn api_post_settings_persists_and_returns_saved_values() {
        let (app, _state, pool) = fixture().await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let body = serde_json::json!({
            "ebook_library_path": "/books/ebooks",
            "audiobook_library_path": "/books/audio"
        });
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/settings")
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let settings: Settings = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            settings.ebook_library_path,
            Some("/books/ebooks".to_string())
        );
        assert_eq!(
            settings.audiobook_library_path,
            Some("/books/audio".to_string())
        );
    }

    #[tokio::test]
    async fn api_get_settings_after_post_reflects_saved_values() {
        let (app, _state, pool) = fixture().await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let body = serde_json::json!({
            "ebook_library_path": "/my/ebooks",
            "audiobook_library_path": null
        });
        app.clone()
            .oneshot(
                Request::builder()
                    .uri("/api/settings")
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("POST should succeed");

        let response = app
            .oneshot(get_with_bearer("/api/settings", &token))
            .await
            .expect("GET should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let settings: Settings = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(settings.ebook_library_path, Some("/my/ebooks".to_string()));
        assert_eq!(settings.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn api_get_library_returns_empty_sections_when_paths_not_configured() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let response = app
            .oneshot(get_with_bearer("/api/library", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let contents: omnibus_shared::LibraryContents = serde_json::from_slice(&bytes).unwrap();
        assert!(contents.ebooks.path.is_none());
        assert_eq!(contents.ebooks.total_files, 0);
        assert!(contents.audiobooks.path.is_none());
        assert_eq!(contents.audiobooks.total_files, 0);
    }

    #[tokio::test]
    async fn api_get_library_reports_error_for_nonexistent_path() {
        let (_, _, pool) = fixture().await;
        db::set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/does/not/exist/omnibus_test".to_string()),
                audiobook_library_path: None,
            },
        )
        .await
        .expect("set should succeed");
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let app = rest_router(AppState::new(pool));

        let response = app
            .oneshot(get_with_bearer("/api/library", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let contents: omnibus_shared::LibraryContents = serde_json::from_slice(&bytes).unwrap();
        assert!(contents.ebooks.error.is_some());
        assert!(contents.audiobooks.path.is_none());
    }

    #[tokio::test]
    async fn api_get_ebooks_returns_empty_when_path_not_configured() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let response = app
            .oneshot(get_with_bearer("/api/ebooks", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
        assert!(lib.path.is_none());
        assert!(lib.books.is_empty());
        assert!(lib.error.is_none());
    }

    #[tokio::test]
    async fn api_get_ebooks_returns_empty_library_for_configured_path_without_index() {
        // /api/ebooks now reads from the books table; an unindexed path
        // surfaces as an empty library at that path, not an error.
        let pool = db::init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let path = "/does/not/exist/omnibus_ebook_test";
        db::set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some(path.to_string()),
                audiobook_library_path: None,
            },
        )
        .await
        .expect("set should succeed");
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let app = rest_router(AppState::new(pool));

        let response = app
            .oneshot(get_with_bearer("/api/ebooks", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(lib.path.as_deref(), Some(path));
        assert!(lib.books.is_empty());
        assert!(lib.error.is_none());
    }

    #[tokio::test]
    async fn api_get_ebooks_sets_total_count_header_with_indexed_library() {
        // Issue #81: every /api/ebooks response carries an X-Total-Count
        // header. When the result fits under MAX_BOOKS_RETURNED no
        // X-Total-Cap header is set, so the client knows the response
        // is complete.
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        db::set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/lib".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();
        db::replace_books(
            &pool,
            "/lib",
            vec![
                db::ebook::IndexedBook {
                    metadata: omnibus_shared::EbookMetadata {
                        filename: "alpha.epub".into(),
                        title: Some("A".into()),
                        ..Default::default()
                    },
                    cover: None,
                    mtime_epoch: 0,
                    size_bytes: 0,
                },
                db::ebook::IndexedBook {
                    metadata: omnibus_shared::EbookMetadata {
                        filename: "beta.epub".into(),
                        title: Some("B".into()),
                        ..Default::default()
                    },
                    cover: None,
                    mtime_epoch: 0,
                    size_bytes: 0,
                },
            ],
        )
        .await
        .unwrap();

        let response = app
            .oneshot(get_with_bearer("/api/ebooks", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("X-Total-Count")
                .and_then(|v| v.to_str().ok()),
            Some("2"),
            "X-Total-Count must reflect the true row count"
        );
        assert!(
            response.headers().get("X-Total-Cap").is_none(),
            "X-Total-Cap must not be set when the response is not truncated"
        );
    }

    #[tokio::test]
    async fn api_get_ebooks_sets_total_cap_header_when_truncated() {
        // Issue #81: when the underlying row count exceeds MAX_BOOKS_RETURNED,
        // the response body is capped and X-Total-Cap is attached so the
        // client knows the JSON it received isn't the full set.
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        db::set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/lib".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();

        // Bulk-seed > MAX_BOOKS_RETURNED rows directly so the test runtime
        // stays in milliseconds. The cap behavior only needs rows to exist;
        // the indexer's full m2m wiring isn't relevant here.
        let total = db::MAX_BOOKS_RETURNED + 5;
        let lib_id: i64 = sqlx::query_scalar(
            "INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        sqlx::query(
            r#"
            WITH RECURSIVE n(i) AS (
                SELECT 1
                UNION ALL
                SELECT i + 1 FROM n WHERE i < ?
            )
            INSERT INTO books (uuid, library_id, path, title, sort)
            SELECT 'uuid-' || i, ?, '/lib/b' || i, 'Title ' || i,
                   'Title ' || printf('%010d', i)
              FROM n
            "#,
        )
        .bind(total)
        .bind(lib_id)
        .execute(&pool)
        .await
        .unwrap();

        let response = app
            .oneshot(get_with_bearer("/api/ebooks", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("X-Total-Count")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<i64>().ok()),
            Some(total),
            "X-Total-Count must report the uncapped row count"
        );
        assert_eq!(
            response
                .headers()
                .get("X-Total-Cap")
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<i64>().ok()),
            Some(db::MAX_BOOKS_RETURNED),
            "X-Total-Cap must equal MAX_BOOKS_RETURNED when the response is truncated"
        );
    }

    #[tokio::test]
    async fn api_get_search_sets_total_count_header_with_indexed_library() {
        // Issue #81: /api/search must attach X-Total-Count on every response,
        // matching the /api/ebooks contract.
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        db::set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/lib".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();
        db::replace_books(
            &pool,
            "/lib",
            vec![
                db::ebook::IndexedBook {
                    metadata: omnibus_shared::EbookMetadata {
                        filename: "alpha.epub".into(),
                        title: Some("Alpha".into()),
                        ..Default::default()
                    },
                    cover: None,
                    mtime_epoch: 0,
                    size_bytes: 0,
                },
                db::ebook::IndexedBook {
                    metadata: omnibus_shared::EbookMetadata {
                        filename: "beta.epub".into(),
                        title: Some("Beta".into()),
                        ..Default::default()
                    },
                    cover: None,
                    mtime_epoch: 0,
                    size_bytes: 0,
                },
            ],
        )
        .await
        .unwrap();

        let response = app
            .oneshot(get_with_bearer("/api/search?q=Alpha", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("X-Total-Count")
                .and_then(|v| v.to_str().ok()),
            Some("1"),
            "X-Total-Count must reflect the FTS match count"
        );
        assert!(
            response.headers().get("X-Total-Cap").is_none(),
            "X-Total-Cap must not be set when search results fit under the cap"
        );
    }

    #[tokio::test]
    async fn api_get_search_sets_total_count_zero_when_path_not_configured() {
        // Issue #81: the early-return path (no library configured) must
        // still attach X-Total-Count: 0 so the client can rely on the
        // header always being present.
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let response = app
            .oneshot(get_with_bearer("/api/search?q=anything", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("X-Total-Count")
                .and_then(|v| v.to_str().ok()),
            Some("0"),
            "X-Total-Count must be 0 on the no-library-configured early return"
        );
        assert!(
            response.headers().get("X-Total-Cap").is_none(),
            "X-Total-Cap must not be set on the early-return path"
        );
    }

    #[tokio::test]
    async fn api_get_ebook_returns_200_with_metadata() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        db::replace_books(
            &pool,
            "/lib",
            vec![db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "alpha.epub".into(),
                    title: Some("Alpha Book".into()),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();

        let books = db::list_books(&pool, "/lib").await.unwrap();
        let id = books[0].id;
        let uuid = books[0].unique_identifier.clone().unwrap();

        let response = app
            .oneshot(get_with_bearer(&format!("/api/ebooks/{uuid}"), &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(book.title.as_deref(), Some("Alpha Book"));
        assert_eq!(book.id, id);
    }

    #[tokio::test]
    async fn api_get_ebook_returns_404_for_unknown_id() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let response = app
            .oneshot(get_with_bearer("/api/ebooks/9999", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_get_ebook_returns_401_when_anonymous() {
        let (app, _state, _pool) = fixture().await;
        let response = app
            .oneshot(get_anon("/api/ebooks/1"))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_search_returns_empty_when_path_not_configured() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let response = app
            .oneshot(get_with_bearer("/api/search?q=hello", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
        assert!(lib.path.is_none());
        assert!(lib.books.is_empty());
    }

    #[tokio::test]
    async fn api_search_rejects_missing_q_param() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let response = app
            .oneshot(get_with_bearer("/api/search", &token))
            .await
            .expect("request should succeed");
        // axum's Query extractor returns 400 for missing required fields.
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_get_covers_returns_not_found_for_missing_id() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let response = app
            .oneshot(get_with_bearer("/api/covers/9999", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn post_settings_triggers_scan_via_worker() {
        let (app, state, pool) = fixture().await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        // Copy the playwright fixtures into an RAII temp dir before pointing
        // the indexer at them. Reindex now opts into cover-sidecar
        // materialization (F0.6) and would otherwise write `<stem>.{jpg|png}`
        // into the shared fixtures dir on every CI run. `tempfile::TempDir`
        // cleans itself up on Drop, so a panic before the assert below doesn't
        // leak under /tmp.
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../test_data/epubs/generated")
            .canonicalize()
            .expect("fixtures dir should resolve");
        assert!(source.is_dir(), "fixtures dir missing: {source:?}");
        let scratch = tempfile::tempdir().expect("create scratch dir");
        for entry in std::fs::read_dir(&source).expect("read fixtures dir") {
            let entry = entry.expect("fixture entry");
            if entry.file_type().expect("file type").is_file() {
                let dest = scratch.path().join(entry.file_name());
                std::fs::copy(entry.path(), dest).expect("copy fixture");
            }
        }
        let path_str = scratch.path().to_string_lossy().to_string();

        let body = serde_json::json!({
            "ebook_library_path": path_str,
            "audiobook_library_path": null,
        });
        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri("/api/settings")
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("POST should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let task_id: db::worker::TaskId = response
            .headers()
            .get("X-Omnibus-Worker-Task-Id")
            .expect("worker task id header should be set in debug builds")
            .to_str()
            .expect("header value should be ASCII")
            .parse()
            .expect("header value should be a u64");

        let outcome = state.worker().await_completion(task_id).await;
        assert!(
            matches!(outcome, TaskOutcome::Ok),
            "worker scan should succeed on a valid fixture dir, got {outcome:?}"
        );

        let response = app
            .oneshot(get_with_bearer("/api/ebooks", &token))
            .await
            .expect("GET /api/ebooks should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
        assert!(
            !lib.books.is_empty(),
            "worker should have indexed at least one book from {path_str}"
        );
        // `scratch` (and any cover sidecars the indexer materialized into
        // it) cleans up on Drop here.
    }

    /// Regression test for issue #112: when the worker's scan fails (here,
    /// because the configured library path doesn't exist on disk), the
    /// `/api/reindex` handler must surface the failure as a 500 via the
    /// `internal()` helper rather than panicking the spawned task or
    /// returning a misleading 200. This is the live request path the
    /// original `panic!("worker scan failed: ...")` was masking.
    #[tokio::test]
    async fn reindex_returns_500_when_worker_fails() {
        let (app, _state, pool) = fixture().await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        // Point settings at a path that definitely doesn't exist on disk so
        // the worker's `Task::Scan` returns `TaskOutcome::Err`.
        let bogus_path = std::env::temp_dir()
            .join(format!(
                "omnibus-nonexistent-{}",
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos())
                    .unwrap_or_default(),
            ))
            .to_string_lossy()
            .to_string();
        let settings = omnibus_shared::Settings {
            ebook_library_path: Some(bogus_path),
            audiobook_library_path: None,
        };
        db::set_settings(&pool, &settings)
            .await
            .expect("set_settings should persist the bogus path");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/reindex")
                    .method("POST")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);

        // Body must stay generic — the underlying scan error message is
        // logged via `tracing::error!` but never leaked on the wire (see
        // `internal()`).
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&bytes).unwrap_or("");
        assert_eq!(body, "internal server error");
    }

    #[tokio::test]
    async fn reindex_returns_409_when_no_library_path_configured() {
        let (app, _state, pool) = fixture().await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/reindex")
                    .method("POST")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn reindex_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/reindex")
                    .method("POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn reindex_returns_403_when_not_admin() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "reader").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/reindex")
                    .method("POST")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn reindex_returns_200_when_scan_succeeds() {
        let (app, _state, pool) = fixture().await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        // Same fixture-copy pattern as `post_settings_triggers_scan_via_worker`
        // so the scan finds a real EPUB and `Task::Scan` returns `Ok`. We use
        // a tempdir to keep the reindex from materializing cover sidecars
        // back into the shared fixtures directory.
        let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../test_data/epubs/generated")
            .canonicalize()
            .expect("fixtures dir should resolve");
        let scratch = tempfile::tempdir().expect("create scratch dir");
        for entry in std::fs::read_dir(&source).expect("read fixtures dir") {
            let entry = entry.expect("fixture entry");
            if entry.file_type().expect("file type").is_file() {
                let dest = scratch.path().join(entry.file_name());
                std::fs::copy(entry.path(), dest).expect("copy fixture");
            }
        }
        let settings = omnibus_shared::Settings {
            ebook_library_path: Some(scratch.path().to_string_lossy().to_string()),
            audiobook_library_path: None,
        };
        db::set_settings(&pool, &settings)
            .await
            .expect("set_settings should persist the fixture path");

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/api/reindex")
                    .method("POST")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
    }

    // -------------------------------------------------------------------
    // 401 — anonymous request rejected by the per-route extractor (the
    // top-level `require_auth` middleware is not in this test stack;
    // these assertions confirm the extractor itself enforces the gate).
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_value_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/value")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_value_increment_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/value/increment")
                    .method("POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_get_settings_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/settings")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_post_settings_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let body = serde_json::json!({
            "ebook_library_path": null,
            "audiobook_library_path": null,
        });
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/settings")
                    .method("POST")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_library_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/library")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_ebooks_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/ebooks")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_search_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/search?q=hello")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    // -------------------------------------------------------------------
    // /api/search/palette — search palette (F1.5)
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_search_palette_returns_empty_when_path_not_configured() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let response = app
            .oneshot(get_with_bearer("/api/search/palette?q=hello", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let results: omnibus_shared::PaletteResults = serde_json::from_slice(&bytes).unwrap();
        assert!(results.books.is_empty());
        assert!(results.authors.is_empty());
    }

    #[tokio::test]
    async fn api_search_palette_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app
            .oneshot(get_anon("/api/search/palette?q=hello"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_search_palette_returns_429_after_budget_exceeded() {
        // Issue #124: /api/search/* runs four heavy FTS5 queries per request,
        // so it gets a per-IP fixed-window rate limit. The limit is set to
        // SEARCH_RATE_LIMIT_MAX requests per SEARCH_RATE_LIMIT_WINDOW; the
        // (SEARCH_RATE_LIMIT_MAX + 1)th request from the same principal must
        // be rejected with 429.
        //
        // `oneshot` requests carry no `ConnectInfo<SocketAddr>` extension, so
        // the limiter's IP fallback (`0.0.0.0`) applies — every request in
        // this test shares one bucket, which is exactly what we want.
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        for i in 0..SEARCH_RATE_LIMIT_MAX {
            let res = app
                .clone()
                .oneshot(get_with_bearer("/api/search/palette?q=hello", &token))
                .await
                .expect("request should succeed");
            assert_eq!(
                res.status(),
                StatusCode::OK,
                "request #{} (1-indexed: {}) should be within budget",
                i,
                i + 1
            );
        }

        // The (MAX+1)th request must trip the limiter.
        let over_limit = app
            .clone()
            .oneshot(get_with_bearer("/api/search/palette?q=hello", &token))
            .await
            .expect("request should succeed");
        assert_eq!(
            over_limit.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "request beyond SEARCH_RATE_LIMIT_MAX must return 429",
        );
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
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

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

    #[tokio::test]
    async fn api_covers_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/covers/1")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    // -------------------------------------------------------------------
    // /api/thumbs — thumbnail pipeline endpoint
    // -------------------------------------------------------------------

    /// Seed a book row with `has_cover = 0`. Returns the inserted book id.
    async fn seed_book_no_cover(pool: &sqlx::SqlitePool) -> i64 {
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

    #[tokio::test]
    async fn api_thumbs_returns_400_for_bad_size() {
        let (app, _, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let res = app
            .oneshot(get_with_bearer("/api/thumbs/1/xxl", &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_thumbs_returns_404_for_missing_book() {
        let (app, _, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let res = app
            .oneshot(get_with_bearer("/api/thumbs/9999/md", &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_thumbs_returns_202_for_book_without_cover() {
        let (_, _, pool) = fixture().await;
        // seed_book_no_cover uses this fixed uuid; route is uuid-keyed now.
        let _ = seed_book_no_cover(&pool).await;
        let uuid = "00000000-0000-0000-0000-000000000001";
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let app = rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer(&format!("/api/thumbs/{uuid}/md"), &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn api_thumbs_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/thumbs/1/md")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    // -------------------------------------------------------------------
    // 403 — non-admin authenticated user hits an admin-only route.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_get_settings_returns_403_when_not_admin() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "reader").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let res = app
            .oneshot(get_with_bearer("/api/settings", &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn api_post_settings_returns_403_when_not_admin() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "reader").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let body = serde_json::json!({
            "ebook_library_path": "/evil/path",
            "audiobook_library_path": null,
        });
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/settings")
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    // -------------------------------------------------------------------
    // 500 — handler error path returns a generic body, never leaks the
    // underlying sqlx error message. Regression test for #78.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_value_500_body_is_generic_and_never_leaks_db_details() {
        let (_, _, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        // Force `db::get_value` to fail by dropping the table it reads.
        // Auth setup above completed before the drop, so the request still
        // passes the `AuthUser` extractor and reaches `get_value`.
        sqlx::query("DROP TABLE app_state")
            .execute(&pool)
            .await
            .expect("drop app_state");

        let app = rest_router(AppState::new(pool));
        let res = app
            .oneshot(get_with_bearer("/api/value", &token))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);

        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&bytes).expect("utf-8 body");
        assert_eq!(body, "internal server error");
        assert!(
            !body.contains("app_state") && !body.contains("sqlx") && !body.contains("SQL"),
            "500 body must not leak internal error details, got {body:?}"
        );
    }

    // -------------------------------------------------------------------
    // Discovery endpoints: /api/authors/:id, /api/series/:id, /api/tags
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_get_author_returns_200_with_detail() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        // Seed a book with an author via replace_books.
        db::replace_books(
            &pool,
            "/lib",
            vec![db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "test.epub".into(),
                    title: Some("Test Book".into()),
                    creators: vec![omnibus_shared::Contributor {
                        name: "Jane Austen".into(),
                        role: Some("aut".into()),
                        file_as: Some("Austen, Jane".into()),
                        id: None,
                    }],
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();

        // Look up the author id from the DB.
        let author_id: i64 =
            sqlx::query_scalar("SELECT id FROM authors WHERE name = 'Jane Austen'")
                .fetch_one(&pool)
                .await
                .expect("author should exist");

        let response = app
            .oneshot(get_with_bearer(
                &format!("/api/authors/{author_id}"),
                &token,
            ))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let detail: omnibus_shared::AuthorDetail = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(detail.name, "Jane Austen");
        assert_eq!(detail.book_count, 1);
        assert_eq!(detail.books.len(), 1);
        assert_eq!(detail.books[0].title.as_deref(), Some("Test Book"));
    }

    #[tokio::test]
    async fn api_get_author_returns_404_for_unknown_id() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let response = app
            .oneshot(get_with_bearer("/api/authors/9999", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_get_author_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/authors/1")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_get_series_returns_200_with_detail() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        // Seed a book with a series.
        db::replace_books(
            &pool,
            "/lib",
            vec![db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "series-book.epub".into(),
                    title: Some("Dune".into()),
                    series: Some("Dune Chronicles".into()),
                    series_index: Some("1".into()),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();

        let series_id: i64 =
            sqlx::query_scalar("SELECT id FROM series WHERE name = 'Dune Chronicles'")
                .fetch_one(&pool)
                .await
                .expect("series should exist");

        let response = app
            .oneshot(get_with_bearer(&format!("/api/series/{series_id}"), &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let detail: omnibus_shared::SeriesDetail = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(detail.name, "Dune Chronicles");
        assert_eq!(detail.book_count, 1);
        assert_eq!(detail.books.len(), 1);
        assert_eq!(detail.books[0].title.as_deref(), Some("Dune"));
    }

    #[tokio::test]
    async fn api_get_series_returns_404_for_unknown_id() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let response = app
            .oneshot(get_with_bearer("/api/series/9999", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_get_series_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/series/1")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_get_tags_returns_200_with_tag_weights() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        // Seed a book with subjects (tags).
        db::replace_books(
            &pool,
            "/lib",
            vec![db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "tagged.epub".into(),
                    title: Some("Tagged Book".into()),
                    subjects: vec!["Fiction".into(), "Science".into()],
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();

        let response = app
            .oneshot(get_with_bearer("/api/tags", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let tags: Vec<omnibus_shared::TagWeight> = serde_json::from_slice(&bytes).unwrap();
        assert!(!tags.is_empty());
        assert!(tags.iter().any(|t| t.name == "Fiction"));
        assert!(tags.iter().any(|t| t.name == "Science"));
        // Each tag should have count = 1 since we seeded one book.
        for tag in &tags {
            assert_eq!(tag.count, 1);
        }
    }

    #[tokio::test]
    async fn api_get_tags_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/tags")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    // -------------------------------------------------------------------
    // F1.12 — /api/authors and /api/series index endpoints.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_get_authors_index_returns_summaries_scoped_to_library() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        // Point settings at /lib and seed two books with distinct authors.
        db::set_settings(
            &pool,
            &omnibus_shared::Settings {
                ebook_library_path: Some("/lib".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        db::replace_books(
            &pool,
            "/lib",
            vec![
                db::ebook::IndexedBook {
                    metadata: omnibus_shared::EbookMetadata {
                        filename: "a.epub".into(),
                        title: Some("A".into()),
                        creators: vec![omnibus_shared::Contributor {
                            name: "Aaron Albright".into(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    cover: None,
                    mtime_epoch: 0,
                    size_bytes: 0,
                },
                db::ebook::IndexedBook {
                    metadata: omnibus_shared::EbookMetadata {
                        filename: "b.epub".into(),
                        title: Some("B".into()),
                        creators: vec![omnibus_shared::Contributor {
                            name: "Zelda Zinn".into(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    },
                    cover: None,
                    mtime_epoch: 0,
                    size_bytes: 0,
                },
            ],
        )
        .await
        .unwrap();

        let response = app
            .oneshot(get_with_bearer("/api/authors", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let authors: Vec<omnibus_shared::AuthorSummary> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(authors.len(), 2);
        let names: Vec<_> = authors.iter().map(|a| a.name.clone()).collect();
        assert_eq!(
            names,
            vec!["Aaron Albright".to_string(), "Zelda Zinn".to_string()],
            "expected alpha order"
        );
        for a in &authors {
            assert_eq!(a.book_count, 1);
        }
    }

    #[tokio::test]
    async fn api_get_authors_index_returns_empty_when_no_library_configured() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let response = app
            .oneshot(get_with_bearer("/api/authors", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let authors: Vec<omnibus_shared::AuthorSummary> = serde_json::from_slice(&bytes).unwrap();
        assert!(authors.is_empty());
    }

    #[tokio::test]
    async fn api_get_authors_index_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/authors")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_get_series_index_returns_summaries_with_primary_author() {
        let (app, _state, pool) = fixture().await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        db::set_settings(
            &pool,
            &omnibus_shared::Settings {
                ebook_library_path: Some("/lib".into()),
                ..Default::default()
            },
        )
        .await
        .unwrap();
        db::replace_books(
            &pool,
            "/lib",
            vec![db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "dune.epub".into(),
                    title: Some("Dune".into()),
                    series: Some("Dune Chronicles".into()),
                    series_index: Some("1".into()),
                    creators: vec![omnibus_shared::Contributor {
                        name: "Frank Herbert".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();

        let response = app
            .oneshot(get_with_bearer("/api/series", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let series: Vec<omnibus_shared::SeriesSummary> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].name, "Dune Chronicles");
        assert_eq!(series[0].book_count, 1);
        assert_eq!(series[0].primary_author.as_deref(), Some("Frank Herbert"));
    }

    #[tokio::test]
    async fn api_get_series_index_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/series")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
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
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

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

    // -------------------------------------------------------------------
    // F5.1 metadata-override REST endpoints (issue #105).
    //
    // The RPC variants (`rpc_save_overrides`, `rpc_delete_overrides`) are
    // covered by DB-level unit tests in `db::queries`; these integration
    // tests cover the REST entry points the mobile client uses.
    // -------------------------------------------------------------------

    /// Seed a single book with a known title via `replace_books` and return
    /// its `id`. The book's stable UUID can be looked up afterwards via
    /// `list_books` if a test needs to assert against the overrides row
    /// directly.
    async fn seed_book(pool: &sqlx::SqlitePool, library: &str, title: &str) -> i64 {
        seed_book_with_uuid(pool, library, title).await.0
    }

    /// Same as `seed_book` but returns `(id, uuid)`. New tests that build
    /// uuid-keyed URLs (covers, thumbs, ebooks, overrides) use this.
    async fn seed_book_with_uuid(
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
    fn build_cover_multipart(content_type: &str, bytes: &[u8]) -> (String, Vec<u8>) {
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
    const TINY_PNG: &[u8] = &[
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
    static COVER_DIR_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    /// RAII guard that points `OMNIBUS_COVERS_DIR` at a fresh scratch dir
    /// for the duration of a single test and restores the previous value
    /// (or removes the var) on drop. Holds the `COVER_DIR_ENV_LOCK` so
    /// parallel cover tests serialize their env-var writes.
    struct CoversDirGuard {
        path: std::path::PathBuf,
        prev: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl CoversDirGuard {
        fn new(tag: &str) -> Self {
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

    #[tokio::test]
    async fn api_post_overrides_requires_auth() {
        let (app, _state, _pool) = fixture().await;
        let body = serde_json::json!({ "title": "Edited" });
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/ebooks/1/overrides")
                    .method("POST")
                    .header("content-type", "application/json")
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_post_overrides_requires_edit_permission() {
        // A plain user from `create_user` has `can_edit = false`, so the
        // handler's per-route check must reject them with 403 before the
        // override row is touched.
        let (app, _state, pool) = fixture().await;
        let id = seed_book(&pool, "/lib", "Original").await;
        let user = test_support::create_user(&pool, "reader").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let body = serde_json::json!({ "title": "Edited" });
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{id}/overrides"))
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        // No override row should have been written.
        let books = db::list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        assert!(
            db::get_metadata_overrides(&pool, &uuid)
                .await
                .unwrap()
                .is_none(),
            "403 path must not persist any override"
        );
    }

    #[tokio::test]
    async fn api_post_overrides_saves_and_returns_merged_book() {
        // Admin (which carries `can_edit = true` via test_support::create_admin)
        // POSTs an override. The handler must persist it, return the merged
        // book, and flip `has_override` on the response.
        let (app, _state, pool) = fixture().await;
        let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Original").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let body = serde_json::json!({
            "title": "Edited Title",
            "publisher": "Edited Publisher",
        });
        let response = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/overrides"))
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(book.id, id);
        assert_eq!(book.title.as_deref(), Some("Edited Title"));
        assert_eq!(book.publisher.as_deref(), Some("Edited Publisher"));
        assert!(
            book.has_override,
            "merged book should advertise has_override = true"
        );

        // The override row must reflect the saved fields.
        let books = db::list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        let (saved, has_cover) = db::get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .expect("override row should exist after POST");
        assert_eq!(saved.title.as_deref(), Some("Edited Title"));
        assert_eq!(saved.publisher.as_deref(), Some("Edited Publisher"));
        assert!(!has_cover, "text-only edit must not set has_cover_override");
    }

    #[tokio::test]
    async fn api_delete_overrides_reverts() {
        // Persist an override via the same REST path the client uses, then
        // delete it and assert the response reflects the canonical scanned
        // values (no `Option` overrides applied).
        let (app, _state, pool) = fixture().await;
        let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Original").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let post_body = serde_json::json!({ "title": "Edited" });
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/overrides"))
                    .method("POST")
                    .header("content-type", "application/json")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(post_body.to_string()))
                    .unwrap(),
            )
            .await
            .expect("POST should succeed");
        assert_eq!(res.status(), StatusCode::OK);

        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/overrides"))
                    .method("DELETE")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("DELETE should succeed");
        assert_eq!(res.status(), StatusCode::OK);

        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(book.id, id);
        assert_eq!(
            book.title.as_deref(),
            Some("Original"),
            "delete must revert to the scanned title"
        );
        assert!(
            !book.has_override,
            "delete must clear the has_override flag on the merged book"
        );

        // And the override row must be gone from the DB.
        let books = db::list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        assert!(
            db::get_metadata_overrides(&pool, &uuid)
                .await
                .unwrap()
                .is_none(),
            "delete must drop the metadata_overrides row"
        );
    }

    #[tokio::test]
    async fn api_post_cover_upload_replaces_cover() {
        // End-to-end happy path: admin POSTs a valid PNG as multipart and
        // the handler writes the override cover file, flips
        // `has_cover_override` on the row, and returns the merged book with
        // `has_override = true`.
        let _covers = CoversDirGuard::new("upload_replaces");
        let (app, _state, pool) = fixture().await;
        let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "CoverBook").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let (content_type, body) = build_cover_multipart("image/png", TINY_PNG);
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/cover"))
                    .method("POST")
                    .header("content-type", content_type)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::OK);

        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let book: omnibus_shared::EbookMetadata = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(book.id, id);
        assert!(
            book.has_override,
            "uploading a cover must mark the merged book as overridden"
        );

        // The override row must record `has_cover_override = 1` and the
        // PNG bytes must be on disk under the scratch covers dir.
        let books = db::list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        let (_, has_cover_override) = db::get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .expect("override row should exist after cover upload");
        assert!(
            has_cover_override,
            "cover upload must set has_cover_override = 1"
        );
        let override_path = db::covers_dir().join(format!("override-{uuid}.png"));
        let on_disk = std::fs::read(&override_path).expect("override cover file should be on disk");
        assert_eq!(on_disk, TINY_PNG);
    }

    #[tokio::test]
    async fn api_post_cover_rejects_non_image() {
        // A multipart `cover` field whose Content-Type is `text/plain` must
        // be rejected at the `starts_with("image/")` guard with 400.
        let _covers = CoversDirGuard::new("non_image");
        let (app, _state, pool) = fixture().await;
        let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "NonImageBook").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let (content_type, body) = build_cover_multipart("text/plain", b"not an image");
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/cover"))
                    .method("POST")
                    .header("content-type", content_type)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        // No override row should have been written for the rejected upload.
        let books = db::list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        assert!(
            db::get_metadata_overrides(&pool, &uuid)
                .await
                .unwrap()
                .is_none(),
            "rejected non-image upload must not create an override row"
        );
    }

    #[tokio::test]
    async fn api_post_cover_rejects_oversized() {
        // The per-route layer raises the body cap to 11 MiB so the handler
        // can enforce its own 10 MB content cap with a clean 400 instead of
        // the framework's 413. Build a payload just over 10 MB to trip that
        // handler-level check.
        let _covers = CoversDirGuard::new("oversized");
        let (app, _state, pool) = fixture().await;
        let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "OversizedBook").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        // 10 MiB + 1 KiB of PNG-prefixed bytes — passes magic-byte
        // detection so we reach the size check, not the format check.
        let mut payload = TINY_PNG.to_vec();
        payload.resize(10 * 1024 * 1024 + 1024, 0);
        let (content_type, body) = build_cover_multipart("image/png", &payload);
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/cover"))
                    .method("POST")
                    .header("content-type", content_type)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);

        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&bytes).unwrap_or("");
        assert!(
            body.contains("10 MB"),
            "400 body should explain the size cap, got {body:?}"
        );
    }

    #[tokio::test]
    async fn api_post_cover_rejects_undetectable_format() {
        // Content-Type passes the `image/` prefix gate but the bytes carry no
        // recognisable image magic header, so `detect_image_format` returns
        // `None`. The handler must surface a 415 (#210) instead of
        // `.unwrap()`-panicking the task into a bare 500.
        let _covers = CoversDirGuard::new("undetectable_format");
        let (app, _state, pool) = fixture().await;
        let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "UndetectableBook").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        // image/png header, but the body is not a real image.
        let (content_type, body) =
            build_cover_multipart("image/png", b"definitely not image bytes");
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{uuid}/cover"))
                    .method("POST")
                    .header("content-type", content_type)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&bytes).unwrap_or("");
        assert!(
            body.contains("Could not detect image format"),
            "415 body should explain the format detection failure, got {body:?}"
        );

        // No override row should have been written for the rejected upload.
        let books = db::list_books(&pool, "/lib").await.unwrap();
        let uuid = books[0].unique_identifier.clone().unwrap();
        assert!(
            db::get_metadata_overrides(&pool, &uuid)
                .await
                .unwrap()
                .is_none(),
            "rejected undetectable-format upload must not create an override row"
        );
    }

    // ---------------------------------------------------------------------
    // F1.11 Author profile photos.
    // ---------------------------------------------------------------------

    async fn seed_author(pool: &sqlx::SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar::<_, i64>("INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id")
            .bind(name)
            .bind(name)
            .fetch_one(pool)
            .await
            .expect("seed author")
    }

    /// Multipart body with one `photo` field, matching `build_cover_multipart`
    /// but using the field name the author-photo handler expects.
    fn build_photo_multipart(content_type: &str, bytes: &[u8]) -> (String, Vec<u8>) {
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

    #[tokio::test]
    async fn api_get_author_photo_requires_auth() {
        let (app, _state, _pool) = fixture().await;
        let res = app
            .oneshot(get_anon("/api/authors/1/photo"))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_get_author_photo_404_when_unset() {
        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;
        let res = app
            .oneshot(get_with_bearer(&format!("/api/authors/{id}/photo"), &token))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_put_author_photo_requires_admin() {
        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let (content_type, body) = build_photo_multipart("image/png", TINY_PNG);
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/authors/{id}/photo"))
                    .method("PUT")
                    .header("content-type", content_type)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn api_put_author_photo_uploads_and_get_serves() {
        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let (content_type, body) = build_photo_multipart("image/png", TINY_PNG);
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/authors/{id}/photo"))
                    .method("PUT")
                    .header("content-type", content_type)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        // GET should now return the uploaded bytes with the detected mime.
        let res = app
            .clone()
            .oneshot(get_with_bearer(&format!("/api/authors/{id}/photo"), &token))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::OK);
        let ct = res
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|h| h.to_str().ok())
            .unwrap_or("")
            .to_string();
        assert_eq!(ct, "image/png");
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(bytes.as_ref(), TINY_PNG);

        // The author detail payload must now flag has_photo = true.
        let res = app
            .oneshot(get_with_bearer(&format!("/api/authors/{id}"), &token))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let author: omnibus_shared::AuthorDetail = serde_json::from_slice(&bytes).unwrap();
        assert!(author.has_photo, "has_photo should flip after upload");
    }

    #[tokio::test]
    async fn api_put_author_photo_404_for_missing_author() {
        let (app, _state, pool) = fixture().await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let (content_type, body) = build_photo_multipart("image/png", TINY_PNG);
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/authors/9999/photo")
                    .method("PUT")
                    .header("content-type", content_type)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_put_author_photo_rejects_non_image() {
        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let (content_type, body) = build_photo_multipart("text/plain", b"not an image");
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/authors/{id}/photo"))
                    .method("PUT")
                    .header("content-type", content_type)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_put_author_photo_rejects_bogus_image_bytes() {
        // Content-Type says image/png but the bytes don't carry the PNG magic
        // header — the magic-byte check must catch this even when the
        // declared MIME passes the `image/` prefix guard. The handler surfaces
        // a 415 (#210) since the payload is not a recognisable image format.
        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let (content_type, body) = build_photo_multipart("image/png", b"not really a png");
        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/authors/{id}/photo"))
                    .method("PUT")
                    .header("content-type", content_type)
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::from(body))
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);

        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body = std::str::from_utf8(&bytes).unwrap_or("");
        assert!(
            body.contains("Could not detect image format"),
            "415 body should explain the format detection failure, got {body:?}"
        );
    }

    #[tokio::test]
    async fn api_delete_author_photo_requires_admin() {
        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/authors/{id}/photo"))
                    .method("DELETE")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn api_delete_author_photo_clears_row() {
        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        db::upsert_author_photo(
            &pool,
            id,
            db::AuthorPhotoSource::Manual,
            None,
            Some("image/png"),
            Some(TINY_PNG),
        )
        .await
        .unwrap();

        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/authors/{id}/photo"))
                    .method("DELETE")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        assert!(db::author_photo_status(&pool, id).await.unwrap().is_none());
    }

    // --- Set-by-URL admin handler. The remote fetch is exercised via a
    // local `wiremock` server, never the public internet. Covers the
    // admin gate, the validation paths (404 missing author, 400 empty /
    // bad-scheme URL, non-image content-type, bogus magic bytes), and the
    // happy path (204 then GET returns the bytes).

    /// JSON PUT helper for the `/api/authors/:id/photo/url` route.
    fn put_photo_url(uri: &str, token: &str, url: &str) -> Request<Body> {
        Request::builder()
            .uri(uri)
            .method("PUT")
            .header("content-type", "application/json")
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .body(Body::from(serde_json::json!({ "url": url }).to_string()))
            .unwrap()
    }

    #[tokio::test]
    async fn api_put_author_photo_url_requires_admin() {
        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let res = app
            .oneshot(put_photo_url(
                &format!("/api/authors/{id}/photo/url"),
                &token,
                "http://127.0.0.1:1/never-reached",
            ))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn api_put_author_photo_url_404_for_missing_author() {
        let (app, _state, pool) = fixture().await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let res = app
            .oneshot(put_photo_url(
                "/api/authors/9999/photo/url",
                &token,
                "http://127.0.0.1:1/never-reached",
            ))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_put_author_photo_url_rejects_empty_url() {
        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let res = app
            .oneshot(put_photo_url(
                &format!("/api/authors/{id}/photo/url"),
                &token,
                "   ",
            ))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_put_author_photo_url_rejects_bad_scheme() {
        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        // ftp:// trips the `fetch_remote_image` scheme guard before any
        // outbound request fires.
        let res = app
            .oneshot(put_photo_url(
                &format!("/api/authors/{id}/photo/url"),
                &token,
                "ftp://example.com/photo.jpg",
            ))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_put_author_photo_url_uploads_and_get_serves() {
        use wiremock::{
            matchers::{method, path},
            Mock, MockServer, ResponseTemplate,
        };

        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/portrait.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(TINY_PNG),
            )
            .mount(&mock)
            .await;

        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let url = format!("{}/portrait.png", mock.uri());
        let res = app
            .clone()
            .oneshot(put_photo_url(
                &format!("/api/authors/{id}/photo/url"),
                &token,
                &url,
            ))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::NO_CONTENT);

        let res = app
            .oneshot(get_with_bearer(&format!("/api/authors/{id}/photo"), &token))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        assert_eq!(bytes.as_ref(), TINY_PNG);
    }

    #[tokio::test]
    async fn api_put_author_photo_url_rejects_non_image_content_type() {
        use wiremock::{
            matchers::{method, path},
            Mock, MockServer, ResponseTemplate,
        };

        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/not-a-photo.html"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "text/html")
                    .set_body_string("<html>nope</html>"),
            )
            .mount(&mock)
            .await;

        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let url = format!("{}/not-a-photo.html", mock.uri());
        let res = app
            .oneshot(put_photo_url(
                &format!("/api/authors/{id}/photo/url"),
                &token,
                &url,
            ))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_put_author_photo_url_rejects_bogus_image_bytes() {
        // Server lies — declares image/png but the bytes don't carry the
        // PNG magic header. The handler-side `detect_image_format` sniff
        // must catch this even though the content-type passes the
        // `image/` prefix gate.
        use wiremock::{
            matchers::{method, path},
            Mock, MockServer, ResponseTemplate,
        };

        let mock = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/fake.png"))
            .respond_with(
                ResponseTemplate::new(200)
                    .insert_header("content-type", "image/png")
                    .set_body_bytes(b"definitely not png bytes" as &[u8]),
            )
            .mount(&mock)
            .await;

        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let url = format!("{}/fake.png", mock.uri());
        let res = app
            .oneshot(put_photo_url(
                &format!("/api/authors/{id}/photo/url"),
                &token,
                &url,
            ))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    // --- Scan-for-picture admin gate / not-found contract. The resolver
    // itself is exercised by the wiremock-backed tests in
    // `omnibus_db::author_photos::tests`; these only cover the wiring so
    // they don't reach the real Open Library service.

    #[tokio::test]
    async fn api_scan_author_photo_requires_admin() {
        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/authors/{id}/photo/scan"))
                    .method("POST")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn api_scan_author_photo_404_for_missing_author() {
        let (app, _state, pool) = fixture().await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;

        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/authors/9999/photo/scan")
                    .method("POST")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_scan_author_photo_requires_auth() {
        let (app, _state, _pool) = fixture().await;
        let res = app
            .oneshot(
                Request::builder()
                    .uri("/api/authors/1/photo/scan")
                    .method("POST")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_scan_author_photo_preserves_manual_upload() {
        // Roadmap: manual override wins over the resolver. An admin clicking
        // "Scan for picture" on an author who already has a manual upload
        // must not wipe that upload — the scan handler treats the row as a
        // sticky override and returns resolved=true without deleting.
        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Ada Lovelace").await;
        let admin = test_support::create_admin(&pool, "admin").await;
        let token = test_support::bearer_token(&pool, admin.id).await;
        db::upsert_author_photo(
            &pool,
            id,
            db::AuthorPhotoSource::Manual,
            None,
            Some("image/png"),
            Some(TINY_PNG),
        )
        .await
        .unwrap();

        let res = app
            .oneshot(
                Request::builder()
                    .uri(format!("/api/authors/{id}/photo/scan"))
                    .method("POST")
                    .header(AUTHORIZATION, format!("Bearer {token}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let body: omnibus_shared::AuthorPhotoScanResult = serde_json::from_slice(&bytes).unwrap();
        assert!(
            body.resolved,
            "scan on manual upload should report resolved=true"
        );

        // Manual row must still be intact (same source, same bytes).
        let (src, _) = db::author_photo_status(&pool, id).await.unwrap().unwrap();
        assert_eq!(src, db::AuthorPhotoSource::Manual);
        let (_, served) = db::get_author_photo(&pool, id).await.unwrap().unwrap();
        assert_eq!(served, TINY_PNG, "manual photo bytes must be preserved");
    }

    #[tokio::test]
    async fn api_get_author_response_carries_has_photo_flag() {
        // F1.11 autoresolution wiring lives behind the GET handler — the
        // worker call itself is fire-and-forget so we can't deterministically
        // observe it from a test without a network. What we can verify is
        // that the handler still returns the expected `AuthorDetail` shape
        // with `has_photo = false` when no row exists, and `true` after a
        // manual upload (covered by `api_put_author_photo_uploads_and_get_serves`
        // for the positive case).
        let (app, _state, pool) = fixture().await;
        let id = seed_author(&pool, "Brandon Sanderson").await;
        let user = test_support::create_user(&pool, "alice").await;
        let token = test_support::bearer_token(&pool, user.id).await;

        let res = app
            .oneshot(get_with_bearer(&format!("/api/authors/{id}"), &token))
            .await
            .expect("request should succeed");
        assert_eq!(res.status(), StatusCode::OK);
        let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let author: omnibus_shared::AuthorDetail = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(author.id, id);
        assert!(!author.has_photo, "no row yet means has_photo = false");
    }
}
