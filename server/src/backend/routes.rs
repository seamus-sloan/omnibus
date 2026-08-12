//! Route-group builder functions merged into the top-level `/api/*` router
//! by [`super::rest_router_with_search_limiter`]. Each function owns one
//! feature area; the grouping used to be carried as inline comments in a
//! single flat route list.

use std::sync::Arc;

use axum::{
    routing::{delete, get, patch, post, put},
    Router,
};

use super::{
    account, admin_health, admin_sessions, audiobooks, author_photos, authors, bookmarks, covers,
    cross_format, ebooks, genres, highlights, journals, kindle, overrides, physical, profile,
    progress, ratings, read_status, scan, search, series, settings, shelves, stats, suggestions,
    summary, tags, uploads, users, AppState,
};
use crate::rate_limit::{rate_limit_by_ip, RateLimiter};

/// Health check, settings, ebooks, and audiobook playback routes.
pub(super) fn content_routes() -> Router<AppState> {
    Router::new()
        .route("/api/_health", get(super::health::get_health))
        .route("/api/settings", get(settings::get_settings))
        .route("/api/settings", post(settings::post_settings))
        // Admin write only; the public read is GET /api/auth/registration.
        .route(
            "/api/settings/registration",
            post(settings::post_registration),
        )
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
        // Admin device & session management (F5.4, #910) — all AdminUser-gated.
        .route(
            "/api/admin/users/{id}/sessions",
            get(admin_sessions::get_user_sessions),
        )
        .route(
            "/api/admin/users/{id}/devices",
            get(admin_sessions::get_user_devices),
        )
        .route(
            "/api/admin/sessions/{id}",
            delete(admin_sessions::delete_session),
        )
        .route(
            "/api/admin/devices/{id}",
            delete(admin_sessions::delete_device),
        )
        // Admin server-health report (F5, #952) — index status, worker
        // queue, FTS health, storage, and last errors in one request. Web
        // instead hits the analogous `rpc_get_admin_health` server fn.
        .route("/api/admin/health", get(admin_health::get_admin_health))
        .route("/api/library", get(ebooks::get_library))
        .route(
            "/api/downloads/validators",
            post(ebooks::post_download_validators),
        )
        .route("/api/ebooks", get(ebooks::get_ebooks))
        .route("/api/ebooks/{uuid}", get(ebooks::get_ebook_by_uuid))
        .route("/api/ebooks/{uuid}/file", get(ebooks::get_ebook_file))
        .route(
            "/api/ebooks/{uuid}/pages/{page}",
            get(ebooks::get_ebook_page),
        )
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
///
/// Each helper below owns one feature area (the grouping the route list used
/// to carry as inline comments); this function just merges them.
pub(super) fn data_routes(search_limiter: Arc<RateLimiter>) -> Router<AppState> {
    Router::new()
        .merge(metadata_override_routes())
        .merge(progress_routes())
        .merge(highlight_routes())
        .merge(bookmark_routes())
        .merge(engagement_routes())
        .merge(check_in_routes())
        .merge(physical_collection_routes())
        .merge(shelf_routes())
        .merge(journal_routes())
        .merge(upload_router())
        .merge(book_upload_router())
        .merge(search_router(search_limiter))
        .merge(discovery_routes())
        .merge(suggestion_routes())
        .merge(summary_routes())
        .merge(kindle_routes())
}

/// Metadata override read/write + cover-only revert.
fn metadata_override_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/ebooks/{uuid}/overrides",
            post(overrides::post_ebook_overrides).delete(overrides::delete_ebook_overrides),
        )
        // Cover-only revert. Carries no upload body (unlike the POST in
        // `upload_router`), so it stays outside the upload rate limiter —
        // same split as the author-photo GET/DELETE (`discovery_routes`) vs
        // its PUT (`upload_router`).
        .route(
            "/api/ebooks/{uuid}/cover",
            delete(overrides::delete_ebook_cover),
        )
        // Mirrors the /api/reindex-style admin triggers.
        .route(
            "/api/admin/rewrite-all-epubs",
            post(overrides::post_rewrite_all_epubs),
        )
}

/// F2.1 progress sync — mobile-facing REST. Web hits the analogous
/// `/api/rpc/progress*` server functions defined in `omnibus_frontend::rpc`.
fn progress_routes() -> Router<AppState> {
    Router::new()
        .route("/api/progress", post(progress::post_progress))
        .route("/api/progress/sessions", post(progress::post_sessions))
        .route("/api/progress/recent", get(progress::get_recent_progress))
        .route("/api/progress/{uuid}", get(progress::get_progress))
        .route(
            "/api/books/{uuid}/cross-format-resume",
            get(cross_format::get_cross_format_resume),
        )
        .route(
            "/api/books/{uuid}/alignment",
            get(cross_format::get_alignment),
        )
        .route(
            "/api/books/{uuid}/cross-format-link",
            post(cross_format::post_cross_format_link)
                .delete(cross_format::delete_cross_format_link),
        )
}

/// F2.4b highlight annotations — mobile-facing REST. Web hits the analogous
/// `/api/rpc/highlights/*` server functions.
fn highlight_routes() -> Router<AppState> {
    Router::new()
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
}

/// Bookmarks — mobile-facing REST. Web hits the analogous
/// `/api/rpc/bookmarks/*` server functions. One model serves both the
/// audiobook player and the reader (position = seconds or EPUB CFI).
fn bookmark_routes() -> Router<AppState> {
    Router::new()
        .route("/api/bookmarks", post(bookmarks::post_bookmark))
        .route(
            "/api/bookmarks/book/{book_uuid}",
            get(bookmarks::get_bookmarks),
        )
        .route(
            "/api/bookmarks/{id}",
            put(bookmarks::put_bookmark).delete(bookmarks::delete_bookmark),
        )
}

/// Reading stats, F3.2 star ratings, and F3.4 read/unread state —
/// mobile-facing REST. Web hits the analogous `/api/rpc/stats`,
/// `/api/rpc/ratings/*`, and `/api/rpc/read-status/*` server functions.
fn engagement_routes() -> Router<AppState> {
    Router::new()
        .route("/api/stats", get(stats::get_stats))
        .route("/api/ratings", post(ratings::post_rating))
        .route(
            "/api/ratings/others/{uuid}",
            get(ratings::get_other_ratings),
        )
        .route(
            "/api/ratings/{uuid}",
            get(ratings::get_rating).delete(ratings::delete_rating),
        )
        .route("/api/read-status", put(read_status::put_read_status))
        .route("/api/read-status/{uuid}", get(read_status::get_read_status))
}

/// Physical Check-In scan flow — mobile-facing REST. Web hits the analogous
/// `/api/rpc/scan/*` server functions.
fn check_in_routes() -> Router<AppState> {
    Router::new()
        .route("/api/scan/resolve", post(scan::post_resolve))
        .route("/api/scan/search", post(scan::post_search))
        .route("/api/scan/resolve-meta", post(scan::post_resolve_meta))
        .route("/api/scan/check-in", post(scan::post_check_in))
        .route(
            "/api/scan/physical-only",
            post(scan::post_add_physical_only),
        )
        .route("/api/scan/wishlist", post(scan::post_wishlist_add))
        // Whether a server-wide Google Books key is configured — any
        // authenticated user may read this non-sensitive flag (mirrors
        // `/api/summary/hardcover-configured`). Drives the check-in scan
        // screen's provider note without the admin-only masked-status route.
        .route(
            "/api/scan/google-books-configured",
            get(scan::get_google_books_configured),
        )
        // Google Books API key (admin) — mobile-facing REST. Web hits the
        // analogous `/api/rpc/google-books-key` server fn.
        .route(
            "/api/google-books-key",
            get(scan::get_google_books_key).post(scan::post_google_books_key),
        )
}

/// F Physical Check-In — book-detail collection + wishlist reads/edits.
/// The literal `copies` routes are registered before `/{uuid}` so the
/// param route can't shadow them.
fn physical_collection_routes() -> Router<AppState> {
    Router::new()
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
}

/// F3.1 shelves — mobile-facing REST. Web hits the analogous
/// `/api/rpc/shelves*` server functions. `/preview` and `/containing/{uuid}`
/// are registered before the `{id}` param route so it can't shadow them.
fn shelf_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/shelves",
            get(shelves::list_shelves).post(shelves::create_shelf),
        )
        .route("/api/shelves/preview", post(shelves::preview_rule))
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
}

/// F3.2 public journal entries — mobile-facing REST. Web hits the analogous
/// `/api/rpc/journals/*` server functions. Embedded-image reads are
/// media-gated like covers/thumbs; the matching upload POST lives in
/// `upload_router` (rate-limited, image body cap).
fn journal_routes() -> Router<AppState> {
    Router::new()
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
        .route(
            "/api/journals/images/{name}",
            get(journals::get_journal_image),
        )
}

/// Cover/thumb/author/series/tag discovery routes, including author photo
/// reads. GET/DELETE for author photos carry no upload body (DELETE mutates,
/// but cheaply — it clears photo state, it doesn't ingest one), so they stay
/// outside the rate-limited `upload_router`. Only the binary uploads (cover
/// POST, photo PUT, photo-url PUT) carry the per-IP frequency cap — see
/// `upload_router` (#168).
fn discovery_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/authors/{id}/photo",
            get(author_photos::get_author_photo).delete(author_photos::delete_author_photo),
        )
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
        .route("/api/genres", get(genres::get_genres))
}

/// F3.3 suggestions — mobile-facing REST. Web hits the analogous
/// `/api/rpc/ebook-suggestions` + `/api/rpc/hardcover-key` server fns.
fn suggestion_routes() -> Router<AppState> {
    Router::new()
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
}

/// "Fetch Summary" — mobile-facing REST. Web hits the analogous
/// `/api/rpc/ebook/summary/*` server fns.
fn summary_routes() -> Router<AppState> {
    Router::new()
        .route(
            "/api/ebooks/{uuid}/summary/fetch",
            post(summary::post_ebook_summary_fetch),
        )
        .route("/api/summary/sources", get(summary::get_summary_sources))
}

/// F4.3 Send-to-Kindle — mobile-facing REST. Web hits the analogous
/// `/api/rpc/kindle/send`, `/api/rpc/account/kindle-email`, and
/// `/api/rpc/smtp*` server fns.
fn kindle_routes() -> Router<AppState> {
    Router::new()
        .route("/api/kindle/send", post(kindle::post_send))
        .route("/api/kindle/send/status", get(kindle::get_send_status))
        .route("/api/account/kindle-email", post(kindle::post_kindle_email))
        .route(
            "/api/account/hidden-formats",
            post(account::post_hidden_formats),
        )
        .route("/api/account/profile", post(profile::post_profile))
        .route("/api/account/avatar", delete(profile::delete_avatar))
        .route("/api/users/{user_id}/avatar", get(profile::get_user_avatar))
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
fn search_router(limiter: Arc<RateLimiter>) -> Router<AppState> {
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
/// to the handlers), merged into [`super::rest_router`]. The three routes —
/// cover `POST`, author-photo `PUT`, author-photo-URL `PUT` — each accept a
/// multi-MiB payload and drive disk I/O, WebP transcoding, and SQLite
/// writes, so they share a per-IP budget. The `GET`/`DELETE` author-photo
/// routes carry no upload body (DELETE mutates but cheaply), so they stay
/// in `rest_router`, outside this limiter.
fn upload_router() -> Router<AppState> {
    let limiter = Arc::new(RateLimiter::with_policy(
        super::UPLOAD_RATE_LIMIT_WINDOW,
        super::UPLOAD_RATE_LIMIT_MAX,
    ));
    Router::new()
        .route(
            "/api/ebooks/{uuid}/cover",
            post(overrides::post_ebook_cover),
        )
        .route("/api/journals/images", post(journals::post_journal_image))
        .route("/api/account/avatar", post(profile::post_avatar))
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
    let limiter = Arc::new(RateLimiter::with_policy(
        super::UPLOAD_RATE_LIMIT_WINDOW,
        super::UPLOAD_RATE_LIMIT_MAX,
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
