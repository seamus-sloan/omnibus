//! Server functions callable from the web client.
//!
//! Each `#[get]`/`#[post]` function is compiled in two modes:
//! - On the server (`feature = "server"`) the body executes, accessing the
//!   SQLite pool via an `axum::Extension<SqlitePool>` layered onto the
//!   Dioxus fullstack router in `server/src/main.rs`.
//! - On the web client (`feature = "web"`) the macro generates a fetch-based
//!   stub callable as a normal async function.
//!
//! Mobile does **not** use these. It talks to the hand-written `/api/*`
//! REST routes (see `server/src/backend.rs`) via `reqwest`.
//!
//! These routes are distinct from the REST routes (`/api/rpc/*` vs `/api/*`)
//! so the two clients cannot accidentally collide.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
use omnibus_shared::{
    AuthorDetail, AuthorPhotoScanResult, AuthorSummary, EbookLibrary, EbookMetadata,
    LibraryContents, MetadataOverrides, PaletteResults, SeriesDetail, SeriesSummary, Settings,
    TagWeight, ValueResponse, WorkerStatus,
};

#[cfg(feature = "server")]
use omnibus_db::{self as db, scanner};

// Magic-byte image-format sniff lives in `omnibus-shared` (single source of
// truth shared with `server::backend`). Only the server-side bodies call it.
#[cfg(feature = "server")]
use omnibus_shared::detect_image_format;

/// Server-only extractor alias used by each server function. Only referenced
/// by the server-side body; the `#[cfg(feature = "server")]` stops the
/// web build from importing axum/sqlx types.
#[cfg(feature = "server")]
type PoolExt = dioxus::fullstack::axum::Extension<sqlx::SqlitePool>;

/// Server-only extractor alias for the shared background `Worker`. The
/// fullstack router in `server/src/main.rs` layers it as
/// `Extension<Arc<Worker>>` so server-function bodies can post tasks
/// instead of spawning their own `tokio::spawn` calls.
#[cfg(feature = "server")]
type WorkerExt = dioxus::fullstack::axum::Extension<std::sync::Arc<omnibus_db::worker::Worker>>;

#[cfg(feature = "server")]
pub use server_auth::{AdminUser, AuthUser};

/// Server-side per-route authorization extractors used by the `#[get]` /
/// `#[post]` macros below. These are deliberately scoped to this module
/// instead of imported from `crate::omnibus::auth` — the `frontend` crate
/// can't depend on the `server` crate (cycle), and dioxus already
/// re-exports axum/axum-extra under `dioxus::fullstack::*`, so duplicating
/// ~50 lines is cheaper than restructuring the workspace.
///
/// Behaviour mirrors `server::auth::extractor::AuthUser` /
/// `AdminUser`. Both delegate session validation to
/// `omnibus_db::auth::validate_session`, the single consolidated security
/// surface (token precedence, SHA-256 hashing, absolute + idle expiry,
/// revocation), so the wire-level contract stays in lockstep with the REST
/// side without the compiler being able to catch a divergence.
#[cfg(feature = "server")]
mod server_auth {
    use dioxus::fullstack::axum::extract::FromRequestParts;
    use dioxus::fullstack::axum::http::{header, request::Parts, StatusCode};
    use dioxus::fullstack::axum::response::{IntoResponse, Response};
    use omnibus_db::auth::{self as auth_db, SessionAuthError};
    use sqlx::SqlitePool;

    /// Authenticated user. Extractor returns 401 when no live session is
    /// attached to the request.
    #[derive(Debug, Clone)]
    pub struct AuthUser {
        pub id: i64,
        pub is_admin: bool,
        /// F5.1: metadata edit permission. `true` for the first user (admin)
        /// and any user explicitly granted `can_edit`.
        pub can_edit: bool,
    }

    /// Admin-only wrapper. Extracting this returns 403 for non-admin users
    /// (after a successful `AuthUser` resolution).
    #[derive(Debug, Clone)]
    pub struct AdminUser(pub AuthUser);

    fn unauthorized() -> Response {
        (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }

    fn internal<E: std::fmt::Display>(e: E) -> Response {
        tracing::error!(error = %e, "rpc auth extractor error");
        (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
    }

    impl<S> FromRequestParts<S> for AuthUser
    where
        S: Send + Sync,
    {
        type Rejection = Response;

        async fn from_request_parts(
            parts: &mut Parts,
            _state: &S,
        ) -> Result<Self, Self::Rejection> {
            let pool = parts
                .extensions
                .get::<SqlitePool>()
                .cloned()
                .ok_or_else(|| internal("missing SqlitePool extension"))?;
            let authorization = parts
                .headers
                .get(header::AUTHORIZATION)
                .and_then(|v| v.to_str().ok());
            let cookie_header = parts
                .headers
                .get(header::COOKIE)
                .and_then(|v| v.to_str().ok());
            match auth_db::validate_session(&pool, authorization, cookie_header).await {
                Ok((user, _session)) => Ok(AuthUser {
                    id: user.id,
                    is_admin: user.is_admin,
                    can_edit: user.can_edit,
                }),
                Err(SessionAuthError::Unauthenticated) => Err(unauthorized()),
                Err(SessionAuthError::Internal(e)) => Err(internal(e)),
            }
        }
    }

    impl<S> FromRequestParts<S> for AdminUser
    where
        S: Send + Sync,
    {
        type Rejection = Response;

        async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
            let user = AuthUser::from_request_parts(parts, state).await?;
            if !user.is_admin {
                return Err((StatusCode::FORBIDDEN, "admin required").into_response());
            }
            Ok(AdminUser(user))
        }
    }
}

#[get("/api/rpc/value", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_get_value() -> Result<ValueResponse> {
    let value = db::get_value(&pool.0).await?;
    Ok(ValueResponse { value })
}

#[post("/api/rpc/value/increment", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_increment_value() -> Result<ValueResponse> {
    let value = db::increment_value(&pool.0).await?;
    Ok(ValueResponse { value })
}

#[get("/api/rpc/settings", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_get_settings() -> Result<Settings> {
    Ok(db::get_settings(&pool.0).await?)
}

#[post("/api/rpc/settings", pool: PoolExt, worker: WorkerExt, _admin: AdminUser)]
pub async fn rpc_save_settings(settings: Settings) -> Result<Settings> {
    db::set_settings(&pool.0, &settings).await?;
    let updated = db::get_settings(&pool.0).await?;
    // Library path may have changed (and even when it hasn't, the user has
    // signalled they want to pick up on-disk changes). Hand the reindex
    // off to the shared Worker so concurrent saves serialize per-path.
    if let Some(library_path) = updated.ebook_library_path.clone() {
        worker
            .0
            .post(omnibus_db::worker::Task::Scan { library_path });
    }
    Ok(updated)
}

/// Snapshot of every in-flight and recently-completed background-worker
/// task (F0.5). Polled at 1 Hz by the `WorkerStatusIndicator` component to
/// surface scan / thumbnail / author-photo / (future) cleanup progress
/// under the Save button on `/settings`.
///
/// Auth-gated as `AuthUser` (not `AdminUser`): scans affect the shared
/// library and every authed user has a reason to know one is running.
/// Worker progress snapshot for the in-page indicator.
///
/// Modeled on `rpc_save_settings` because both routes need the
/// `WorkerExt` extension. Empirically the Dioxus fullstack server-fn
/// macro mounts `#[post]` routes whose extractor list contains
/// `WorkerExt`, but the `#[get]` variant of the same signature 404s at
/// runtime (the route string ends up in the binary but never reaches
/// the axum router). The body is idempotent and read-only; using POST
/// is purely a framework workaround so the route actually mounts.
///
/// `_pool: PoolExt` is kept in the extractor list to put the macro on
/// the same code path every other `/api/rpc/*` route uses (all of
/// which take a `PoolExt`), but is unused — no DB calls run.
#[post("/api/rpc/worker_status", _pool: PoolExt, worker: WorkerExt, _user: AuthUser)]
pub async fn rpc_worker_status() -> Result<WorkerStatus> {
    Ok(worker.0.progress_snapshot())
}

#[get("/api/rpc/library", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_get_library() -> Result<LibraryContents> {
    let settings = db::get_settings(&pool.0).await?;
    Ok(scanner::scan_libraries(
        settings.ebook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
    ))
}

#[get("/api/rpc/ebooks", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_get_ebooks() -> Result<EbookLibrary> {
    let settings = db::get_settings(&pool.0).await?;
    // Served straight from the DB — the indexer is responsible for keeping
    // it up to date (startup + settings save triggers).
    //
    // Issue #81: the underlying `list_books` query is capped at
    // `db::MAX_BOOKS_RETURNED` rows so a multi-thousand-book install
    // can't bandwidth-DoS itself on every poll. Dioxus server functions
    // don't expose response headers, so this path can't currently
    // surface a "truncated" hint to the web client — the F1.3 spec
    // constrains the client-side sort/filter UX to ~10k books anyway,
    // which is well under the cap. Cursor pagination is the next step.
    Ok(db::library_from_db(&pool.0, settings.ebook_library_path.as_deref()).await?)
}

/// POST (not GET) for the same reason as `rpc_search`: Dioxus `#[get]`
/// server functions can't carry an argument body, so anything that needs
/// `uuid` rides as a JSON-bodied POST. The argument is the stable
/// `books.uuid` rather than the renumbering `books.id` so bookmarked
/// `/books/:uuid` URLs survive reindexes.
#[post("/api/rpc/ebook", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_get_ebook(uuid: String) -> Result<Option<EbookMetadata>> {
    Ok(db::get_book_by_uuid(&pool.0, &uuid).await?)
}

/// FTS5-backed search across the configured ebook library. Empty or
/// whitespace-only `q` returns an empty library.
///
/// POST (not GET) so the query string can ride in the JSON body — Dioxus
/// `#[get]` server functions reject arg bodies because HTTP spec forbids
/// bodies on GET.
///
/// Issue #81: `search_books` is capped server-side at
/// `db::MAX_BOOKS_RETURNED` hits. Server functions can't expose response
/// headers, so — unlike `rpc_get_ebooks` — the search path threads the full
/// hit count through `EbookLibrary::total` (issue #241), letting the web
/// client show "N of M results" when the vec is truncated.
#[post("/api/rpc/search", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_search(q: String) -> Result<EbookLibrary> {
    let settings = db::get_settings(&pool.0).await?;
    let Some(path) = settings.ebook_library_path else {
        return Ok(EbookLibrary::default());
    };
    // Issue #241: single FTS5 pass returns the capped vec and the full count.
    let (books, total) = db::search_books_with_total(&pool.0, &path, &q).await?;
    Ok(EbookLibrary {
        path: Some(path),
        books,
        error: None,
        total: Some(total),
    })
}

// ---------------------------------------------------------------------------
// Discovery pages (F1.8)
// ---------------------------------------------------------------------------

/// Fetch a single author and all their books. POST for the same reason as
/// `rpc_get_ebook` — needs `id` in the body.
///
/// F1.11: queues a background `Task::ResolveAuthorPhoto` when the author has
/// no `author_photos` row yet so first-time visits trigger Open Library
/// resolution. A subsequent visit picks up the resolved photo (or the
/// `letter` negative-cache marker), and the worker's per-author resource
/// mutex prevents duplicate queueing while resolution is in flight.
#[post("/api/rpc/author", pool: PoolExt, worker: WorkerExt, _user: AuthUser)]
pub async fn rpc_get_author(id: i64) -> Result<Option<AuthorDetail>> {
    let author = db::get_author(&pool.0, id).await?;
    if let Some(ref a) = author {
        if !a.has_photo && db::author_photo_status(&pool.0, id).await?.is_none() {
            worker
                .0
                .post(db::worker::Task::ResolveAuthorPhoto { author_id: id });
        }
    }
    Ok(author)
}

/// Fetch a single series and all its books (ordered by series index).
#[post("/api/rpc/series", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_get_series(id: i64) -> Result<Option<SeriesDetail>> {
    Ok(db::get_series(&pool.0, id).await?)
}

/// F1.11: admin-triggered "Scan for picture" for an author. Clears any
/// sticky `letter` negative-cache marker and runs the Open Library resolver
/// inline, so the admin gets a definitive "found / not found" answer in a
/// single round-trip without polling the worker.
///
/// Manual uploads are treated as overrides: a `source = 'manual'` row is
/// preserved (the F1.11 roadmap explicitly calls this out — "skips if a
/// manual override exists"). Scan returns `resolved=true` in that case
/// without touching the row, so admins can't accidentally wipe a manual
/// upload by clicking the button.
#[post("/api/rpc/author/scan-photo", pool: PoolExt, user: AuthUser)]
pub async fn rpc_scan_author_photo(id: i64) -> Result<AuthorPhotoScanResult> {
    if !user.is_admin {
        return Err(ServerFnError::new("forbidden: admin required").into());
    }
    // Manual uploads win — don't delete or overwrite.
    if let Some((db::AuthorPhotoSource::Manual, _)) = db::author_photo_status(&pool.0, id).await? {
        return Ok(AuthorPhotoScanResult { resolved: true });
    }
    db::delete_author_photo(&pool.0, id).await?;
    db::author_photos::resolve(&pool.0, id).await?;
    let resolved = db::get_author_photo(&pool.0, id).await?.is_some();
    Ok(AuthorPhotoScanResult { resolved })
}

/// F1.11 follow-up: persist an author photo by URL. Admin-gated server-side
/// (the `user.is_admin` check below mirrors `rpc_scan_author_photo`). The
/// server fetches the URL via `db::author_photos::fetch_remote_image`,
/// validates the bytes with the same magic-byte sniff as the multipart
/// upload path, and stores it as a `manual` row.
#[post("/api/rpc/author/photo-url", pool: PoolExt, user: AuthUser)]
pub async fn rpc_set_author_photo_url(id: i64, url: String) -> Result<()> {
    if !user.is_admin {
        return Err(ServerFnError::new("forbidden: admin required").into());
    }
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err(ServerFnError::new("url is required").into());
    }
    let author_exists: bool =
        sqlx::query_scalar::<_, i64>("SELECT EXISTS(SELECT 1 FROM authors WHERE id = ?)")
            .bind(id)
            .fetch_one(&pool.0)
            .await
            .map_err(|e| ServerFnError::new(format!("author exists check: {e}")))?
            != 0;
    if !author_exists {
        return Err(ServerFnError::new("author not found").into());
    }
    let (_mime_hint, bytes) = db::author_photos::fetch_remote_image(trimmed)
        .await
        .map_err(|e| ServerFnError::new(e.to_string()))?;
    let mime = detect_image_format(&bytes)
        .ok_or_else(|| ServerFnError::new("file at URL does not appear to be a valid image"))?;
    db::upsert_author_photo(
        &pool.0,
        id,
        db::AuthorPhotoSource::Manual,
        Some(trimmed),
        Some(&mime),
        Some(&bytes),
    )
    .await
    .map_err(|e| ServerFnError::new(format!("upsert_author_photo: {e}")))?;
    Ok(())
}

/// Return all tags with book counts for the tag cloud.
#[get("/api/rpc/tags", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_get_tag_cloud() -> Result<Vec<TagWeight>> {
    Ok(db::get_tag_cloud(&pool.0).await?)
}

/// F1.12 — `/authors` index: every author in the configured library.
#[get("/api/rpc/authors", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_list_authors() -> Result<Vec<AuthorSummary>> {
    let settings = db::get_settings(&pool.0).await?;
    let Some(path) = settings.ebook_library_path else {
        return Ok(Vec::new());
    };
    Ok(db::list_authors(&pool.0, &path).await?)
}

/// F1.12 — `/series` index: every series in the configured library.
#[get("/api/rpc/series-list", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_list_series() -> Result<Vec<SeriesSummary>> {
    let settings = db::get_settings(&pool.0).await?;
    let Some(path) = settings.ebook_library_path else {
        return Ok(Vec::new());
    };
    Ok(db::list_series(&pool.0, &path).await?)
}

/// Save metadata overrides for a book (F5.1). Requires `can_edit` or admin.
///
/// Returns the merged `EbookMetadata` so the client can update its state
/// without a second round-trip.
#[post("/api/rpc/ebook/overrides", pool: PoolExt, user: AuthUser)]
pub async fn rpc_save_overrides(
    uuid: String,
    overrides: MetadataOverrides,
) -> Result<Option<EbookMetadata>> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    if let Err(msg) = overrides.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    // Merge incoming overrides with any existing ones so that a second edit
    // that only touches field B doesn't wipe a prior override on field A.
    let merged = match db::get_metadata_overrides(&pool.0, &uuid).await? {
        Some((existing, _)) => existing.merge(&overrides),
        None => overrides,
    };
    db::upsert_metadata_overrides(&pool.0, &uuid, &merged, false, user.id).await?;
    Ok(db::get_book_by_uuid(&pool.0, &uuid).await?)
}

/// Delete metadata overrides for a book, reverting to scanned values (F5.1).
#[post("/api/rpc/ebook/overrides/delete", pool: PoolExt, user: AuthUser)]
pub async fn rpc_delete_overrides(uuid: String) -> Result<Option<EbookMetadata>> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    // Resolve uuid → id once so the `invalidate_thumbs` call (id-keyed by
    // the thumbnail pipeline's file layout) stays accurate.
    let Some(book_id) = db::resolve_book_id_by_uuid(&pool.0, &uuid).await? else {
        return Ok(None);
    };
    db::delete_metadata_overrides(&pool.0, &uuid).await?;
    // `delete_override_cover` + `invalidate_thumbs` are sync `std::fs`
    // operations; run them on the blocking pool so this server function
    // doesn't pin a tokio worker thread (#106).
    let uuid_for_blocking = uuid.clone();
    tokio::task::spawn_blocking(move || {
        db::delete_override_cover(&uuid_for_blocking);
        db::thumbs::invalidate_thumbs(book_id);
    })
    .await
    .map_err(|e| ServerFnError::new(format!("spawn_blocking(delete_override_cover): {e}")))?;
    Ok(db::get_book_by_uuid(&pool.0, &uuid).await?)
}

/// F5.9-lite: admin "Delete author" (issue #159). Removes the author row,
/// drops every `books_authors_link` for it, and adds the name to
/// `ignored_authors` so the next `indexer::reindex` does not silently
/// resurrect the row. Returns the number of books that were un-linked
/// (used by the confirmation modal in PR 5).
#[post("/api/rpc/author/delete", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_delete_author(id: i64) -> Result<u64> {
    Ok(db::delete_author(&pool.0, id).await?)
}

/// Search palette — grouped results (books, authors, series, tags) for the
/// command-palette overlay (F1.5).
#[post("/api/rpc/search-palette", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_search_palette(q: String) -> Result<PaletteResults> {
    let settings = db::get_settings(&pool.0).await?;
    let Some(path) = settings.ebook_library_path else {
        return Ok(PaletteResults::default());
    };
    Ok(db::search_palette(&pool.0, &path, &q).await?)
}
