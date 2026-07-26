//! Hand-written `/api/*` REST routes for the mobile client.
//!
//! Web uses Dioxus server functions (see `omnibus_frontend::rpc`), mounted
//! automatically by `dioxus::server::router(App)`. These REST routes are
//! merged alongside them in `main.rs` so mobile's existing `reqwest` paths
//! keep working unchanged.

use std::sync::Arc;

use axum::{
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post, put},
    Extension, Router,
};
use omnibus_db::{
    self as db,
    worker::{Worker, WorkerConfig},
};
use sqlx::SqlitePool;

use crate::rate_limit::{rate_limit_by_ip, RateLimiter};

mod audiobooks;
mod author_photos;
mod authors;
mod bookmarks;
mod covers;
mod ebooks;
mod export_opf;
mod health;
mod highlights;
mod image_upload;
mod journals;
mod kindle;
mod kobo;
mod overrides;
mod physical;
mod progress;
mod ratings;
mod read_status;
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

pub use kobo::kobo_router;

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

/// Serve a file from disk as a browser download, streaming via `ServeFile`
/// (so large audiobook files aren't buffered into memory) and forcing
/// `Content-Disposition: attachment` with the on-disk basename as the
/// suggested filename. `content_type` overrides the guessed MIME. The
/// disposition/content-type headers are only attached to a real file
/// response (200/206); a 404 from `ServeFile` passes through untouched.
async fn serve_download(
    req: axum::extract::Request,
    path: &std::path::Path,
    content_type: &str,
) -> Response {
    use tower::ServiceExt;

    let serve = tower_http::services::ServeFile::new(path);
    let res = match serve.oneshot(req).await {
        Ok(r) => r,
        Err(e) => return internal("serve download", e),
    };
    let (mut parts, body) = res.into_parts();
    let ok = matches!(
        parts.status,
        axum::http::StatusCode::OK | axum::http::StatusCode::PARTIAL_CONTENT
    );
    if ok {
        if let Ok(v) = axum::http::HeaderValue::from_str(content_type) {
            parts.headers.insert(axum::http::header::CONTENT_TYPE, v);
        }
        let filename = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("download");
        if let Ok(v) = axum::http::HeaderValue::from_str(&content_disposition_attachment(filename))
        {
            parts
                .headers
                .insert(axum::http::header::CONTENT_DISPOSITION, v);
        }
    }
    Response::from_parts(parts, axum::body::Body::new(body))
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
    content_routes()
        .merge(data_routes(search_limiter))
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

/// Health check, settings, ebooks, and audiobook playback routes.
fn content_routes() -> Router<AppState> {
    Router::new()
        .route("/api/_health", get(health::get_health))
        .route("/api/settings", get(settings::get_settings))
        .route("/api/settings", post(settings::post_settings))
        .route("/api/reindex", post(settings::post_reindex))
        .route("/api/scan-library", post(settings::post_scan_library))
        .route("/api/fts/rebuild", post(settings::post_rebuild_fts))
        // Admin user management (F5.4) — all AdminUser-gated.
        .route("/api/users", get(users::get_users).post(users::post_user))
        .route("/api/users/{id}", delete(users::delete_user))
        .route(
            "/api/users/{id}/permissions",
            patch(users::patch_permissions),
        )
        .route("/api/users/{id}/password", post(users::post_password))
        .route("/api/users/{id}/unlock", post(users::post_unlock))
        .route("/api/library", get(ebooks::get_library))
        .route("/api/ebooks", get(ebooks::get_ebooks))
        .route("/api/ebooks/{uuid}", get(ebooks::get_ebook_by_uuid))
        .route("/api/ebooks/{uuid}/file", get(ebooks::get_ebook_file))
        .route("/api/ebooks/{uuid}/kepub", get(ebooks::get_ebook_kepub))
        .route(
            "/api/ebooks/{uuid}/download",
            get(ebooks::get_ebook_download),
        )
        .route(
            "/api/audiobooks/{uuid}/download",
            get(audiobooks::get_audiobook_download),
        )
        .route(
            "/api/audiobooks/{uuid}/manifest",
            get(audiobooks::get_audiobook_manifest),
        )
        .route(
            "/api/audiobooks/{uuid}/parts/{ordinal}",
            get(audiobooks::get_audiobook_part),
        )
        .route(
            "/api/audiobooks/{uuid}/playlist.m3u8",
            get(audiobooks::get_audiobook_playlist),
        )
        .route(
            "/api/audiobooks/{uuid}/segments/{segment}",
            get(audiobooks::get_audiobook_segment),
        )
        .route(
            "/api/audiobooks/{uuid}/status",
            get(audiobooks::get_audiobook_status),
        )
        .route(
            "/api/audiobooks/{uuid}/playback-rate",
            get(audiobooks::get_playback_rate).put(audiobooks::put_playback_rate),
        )
}

/// Metadata overrides, progress sync, author/cover/series/tag discovery routes,
/// upload-rate-limited endpoints, and the search sub-router.
fn data_routes(search_limiter: std::sync::Arc<RateLimiter>) -> Router<AppState> {
    Router::new()
        .route(
            "/api/ebooks/{uuid}/overrides",
            post(overrides::post_ebook_overrides).delete(overrides::delete_ebook_overrides),
        )
        .route(
            "/api/ebooks/{uuid}/export-opf",
            post(export_opf::post_export_opf),
        )
        // Cover-only revert. Carries no upload body (unlike the POST in
        // `upload_router`), so it stays outside the upload rate limiter —
        // same split as the author-photo GET/DELETE vs PUT routes below.
        .route(
            "/api/ebooks/{uuid}/cover",
            delete(overrides::delete_ebook_cover),
        )
        // F2.1 progress sync — mobile-facing REST. Web hits the analogous
        // `/api/rpc/progress*` server functions defined in `omnibus_frontend::rpc`.
        .route("/api/progress", post(progress::post_progress))
        .route("/api/progress/sessions", post(progress::post_sessions))
        .route("/api/progress/recent", get(progress::get_recent_progress))
        .route("/api/progress/{uuid}", get(progress::get_progress))
        // F2.4b highlight annotations — mobile-facing REST. Web hits the
        // analogous `/api/rpc/highlights/*` server functions.
        .route("/api/highlights", post(highlights::post_highlight))
        .route(
            "/api/highlights/book/{book_uuid}",
            get(highlights::get_highlights),
        )
        .route(
            "/api/highlights/{id}/color",
            patch(highlights::patch_highlight_color),
        )
        .route(
            "/api/highlights/{id}/note",
            patch(highlights::patch_highlight_note),
        )
        .route("/api/highlights/{id}", delete(highlights::delete_highlight))
        // Bookmarks — mobile-facing REST. Web hits the analogous
        // `/api/rpc/bookmarks/*` server functions. One model serves both the
        // audiobook player and the reader (position = seconds or EPUB CFI).
        .route("/api/bookmarks", post(bookmarks::post_bookmark))
        .route(
            "/api/bookmarks/book/{book_uuid}",
            get(bookmarks::get_bookmarks),
        )
        .route(
            "/api/bookmarks/{id}",
            put(bookmarks::put_bookmark).delete(bookmarks::delete_bookmark),
        )
        // Reading stats — mobile-facing REST. Web hits the analogous
        // `/api/rpc/stats` server function.
        .route("/api/stats", get(stats::get_stats))
        // F3.2 star ratings — mobile-facing REST. Web hits the analogous
        // `/api/rpc/ratings/*` server functions.
        .route("/api/ratings", post(ratings::post_rating))
        .route(
            "/api/ratings/others/{uuid}",
            get(ratings::get_other_ratings),
        )
        .route(
            "/api/ratings/{uuid}",
            get(ratings::get_rating).delete(ratings::delete_rating),
        )
        // F3.4 read/unread state — mobile-facing REST. Web hits the analogous
        // `/api/rpc/read-status/*` server functions.
        .route("/api/read-status", put(read_status::put_read_status))
        .route("/api/read-status/{uuid}", get(read_status::get_read_status))
        // Physical Check-In scan flow — mobile-facing REST. Web hits the
        // analogous `/api/rpc/scan/*` server functions.
        .route("/api/scan/resolve", post(scan::post_resolve))
        .route("/api/scan/check-in", post(scan::post_check_in))
        .route(
            "/api/scan/physical-only",
            post(scan::post_add_physical_only),
        )
        .route("/api/scan/wishlist", post(scan::post_wishlist_add))
        // Google Books API key (admin) — mobile-facing REST. Web hits the
        // analogous `/api/rpc/google-books-key` server fn.
        .route(
            "/api/google-books-key",
            get(scan::get_google_books_key).post(scan::post_google_books_key),
        )
        // F Physical Check-In — book-detail collection + wishlist reads/edits.
        // The literal `copies` routes are registered before `/{uuid}` so the
        // param route can't shadow them.
        .route(
            "/api/physical/copies/{copy_id}",
            patch(physical::patch_copy_note).delete(physical::delete_copy),
        )
        .route("/api/physical/{uuid}/copies", get(physical::get_copies))
        .route(
            "/api/physical/{uuid}/wishlist",
            get(physical::get_wishlist_entry)
                .post(physical::post_wishlist_entry)
                .delete(physical::delete_wishlist_entry),
        )
        .route("/api/physical/{uuid}", delete(physical::delete_book))
        // F3.1 shelves — mobile-facing REST. Web hits the analogous
        // `/api/rpc/shelves*` server functions. `/preview` is registered before
        // the `{id}` param route so it can't be shadowed.
        .route(
            "/api/shelves",
            get(shelves::list_shelves).post(shelves::create_shelf),
        )
        .route("/api/shelves/preview", post(shelves::preview_rule))
        // Also ahead of `{id}`, and for the same reason — `containing` is not
        // a shelf id.
        .route(
            "/api/shelves/containing/{uuid}",
            get(shelves::shelves_containing),
        )
        .route(
            "/api/shelves/{id}",
            get(shelves::get_shelf)
                .patch(shelves::update_shelf)
                .delete(shelves::delete_shelf),
        )
        .route("/api/shelves/{id}/page", get(shelves::get_shelf_page))
        .route("/api/shelves/{id}/books", post(shelves::add_shelf_books))
        .route(
            "/api/shelves/{id}/books/{uuid}",
            delete(shelves::remove_shelf_book),
        )
        // F3.2 public journal entries — mobile-facing REST. Web hits the
        // analogous `/api/rpc/journals/*` server functions.
        .route("/api/journals", post(journals::post_journal))
        .route(
            "/api/journals/book/{book_uuid}",
            get(journals::get_journal_entries),
        )
        .route(
            "/api/journals/preview",
            post(journals::post_journal_preview),
        )
        .route(
            "/api/journals/{id}",
            patch(journals::patch_journal).delete(journals::delete_journal),
        )
        // Embedded-image reads are media-gated like covers/thumbs; the
        // matching upload POST lives in `upload_router` (rate-limited, image
        // body cap).
        .route(
            "/api/journals/images/{name}",
            get(journals::get_journal_image),
        )
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
        .merge(book_upload_router())
        .merge(search_router(search_limiter))
        .route("/api/covers/{uuid}", get(covers::get_cover))
        .route("/api/thumbs/{uuid}/{size}", get(covers::get_thumb))
        .route("/api/authors", get(authors::get_authors))
        .route(
            "/api/authors/refetch-photos",
            post(author_photos::post_refetch_author_photos),
        )
        .route(
            "/api/authors/{id}/photo/scan",
            post(author_photos::post_author_photo_scan),
        )
        .route("/api/authors/{id}", get(authors::get_author_by_id))
        .route("/api/series", get(series::get_series))
        .route("/api/series/{id}", get(series::get_series_by_id))
        .route("/api/tags", get(tags::get_tags))
        // F3.3 suggestions — mobile-facing REST. Web hits the analogous
        // `/api/rpc/ebook-suggestions` + `/api/rpc/hardcover-key` server fns.
        .route(
            "/api/ebooks/{uuid}/suggestions",
            get(suggestions::get_suggestions),
        )
        .route(
            "/api/suggestions/{uuid}/{rank}/cover",
            get(suggestions::get_suggestion_cover),
        )
        .route(
            "/api/hardcover-key",
            get(suggestions::get_hardcover_key).post(suggestions::post_hardcover_key),
        )
        // "Fetch Summary" — mobile-facing REST. Web hits the analogous
        // `/api/rpc/ebook/summary/*` server fns.
        .route(
            "/api/ebooks/{uuid}/summary/fetch",
            post(summary::post_ebook_summary_fetch),
        )
        .route("/api/summary/sources", get(summary::get_summary_sources))
        // F4.3 Send-to-Kindle — mobile-facing REST. Web hits the analogous
        // `/api/rpc/kindle/send`, `/api/rpc/account/kindle-email`, and
        // `/api/rpc/smtp*` server fns.
        .route("/api/kindle/send", post(kindle::post_send))
        .route("/api/kindle/send/status", get(kindle::get_send_status))
        .route("/api/account/kindle-email", post(kindle::post_kindle_email))
        .route("/api/smtp", get(kindle::get_smtp).post(kindle::post_smtp))
        .route("/api/smtp/clear", post(kindle::post_smtp_clear))
        .route("/api/smtp/test", post(kindle::post_smtp_test))
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
/// search families; `rest_router` supplies a fresh dedicated one.
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
/// writes, so they share a per-IP budget. The `GET`/`DELETE` author-photo
/// routes carry no upload body (DELETE mutates but cheaply), so they stay
/// in `rest_router`, outside this limiter.
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
        .route("/api/journals/images", post(journals::post_journal_image))
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

/// Per-IP rate-limited router for the "add your own books" upload endpoints.
/// Kept separate from [`upload_router`] because book/audiobook payloads are far
/// larger than cover/photo images, so these routes carry their own (much
/// higher) `DefaultBodyLimit` from `OMNIBUS_MAX_UPLOAD_BYTES` rather than
/// raising the image routes' 11 MiB cap. Shares the same per-IP frequency
/// budget as the image uploads.
fn book_upload_router() -> Router<AppState> {
    let limiter = std::sync::Arc::new(RateLimiter::with_policy(
        UPLOAD_RATE_LIMIT_WINDOW,
        UPLOAD_RATE_LIMIT_MAX,
    ));
    Router::new()
        .route(
            "/api/uploads/ebooks/inspect",
            post(uploads::post_inspect_ebook),
        )
        .route("/api/uploads/ebooks", post(uploads::post_upload_ebook))
        .route(
            "/api/uploads/audiobooks/inspect",
            post(uploads::post_inspect_audiobook),
        )
        .route(
            "/api/uploads/audiobooks",
            post(uploads::post_upload_audiobook),
        )
        // Book uploads need far more than the global 1 MiB cap; layered closer
        // to the handler so axum picks this larger value for these routes.
        .layer(axum::extract::DefaultBodyLimit::max(
            uploads::max_upload_bytes(),
        ))
        // Outermost: reject an over-budget request before buffering the body.
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

/// Running server's release version, captured once at boot.
///
/// `main.rs` calls [`init_app_version`] eagerly during boot, mirroring
/// [`init_build_id`]/[`init_repo_root`]. `OnceLock::get_or_init` is
/// idempotent, so calling [`app_version`] later returns the same value.
pub fn app_version() -> &'static str {
    APP_VERSION.get_or_init(read_app_version)
}

/// Eagerly initialize [`app_version`]. Idempotent.
pub fn init_app_version() {
    let _ = APP_VERSION.get_or_init(read_app_version);
}

static APP_VERSION: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Reads `OMNIBUS_VERSION` (baked into the Docker image at build time by
/// `docker.yml`) and normalizes it to a single leading `v` — the Docker
/// build-arg carries the full, already-`v`-prefixed release tag, but a
/// hand-set deployment env might not, and a doubled `vv1.2.3` shouldn't
/// happen either.
///
/// Falls back to the literal `"dev"` when the var is unset, empty/whitespace
/// (a build-arg supplied without a value sets the env to `""` rather than
/// leaving it unset), or literally just `"v"` after trimming — local `cargo
/// run`/`dx serve` builds have no release tag to report, and `"dev"` reads
/// unambiguously as "no release tag", never as a real version.
fn read_app_version() -> String {
    let trimmed = std::env::var("OMNIBUS_VERSION")
        .ok()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty());
    match trimmed {
        Some(v) => {
            let bare = v.trim_start_matches('v');
            if bare.is_empty() {
                "dev".to_string()
            } else {
                format!("v{bare}")
            }
        }
        None => "dev".to_string(),
    }
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
