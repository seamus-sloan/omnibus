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
use omnibus_shared::{EbookLibrary, LibraryContents, Settings, ValueResponse};

#[cfg(feature = "server")]
use omnibus_db::{self as db, scanner};

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
/// `AdminUser`. Both call `omnibus_db::auth::parse_session_token` and
/// `lookup_session` so the wire-level token format stays in lockstep with
/// the REST side.
#[cfg(feature = "server")]
mod server_auth {
    use dioxus::fullstack::axum::extract::FromRequestParts;
    use dioxus::fullstack::axum::http::{header, request::Parts, StatusCode};
    use dioxus::fullstack::axum::response::{IntoResponse, Response};
    use omnibus_db::auth::{self as auth_db, AuthError};
    use sqlx::SqlitePool;

    /// Authenticated user. Extractor returns 401 when no live session is
    /// attached to the request.
    #[derive(Debug, Clone)]
    pub struct AuthUser {
        pub id: i64,
        pub is_admin: bool,
    }

    /// Admin-only wrapper. Extracting this returns 403 for non-admin users
    /// (after a successful `AuthUser` resolution).
    #[derive(Debug, Clone)]
    pub struct AdminUser(pub AuthUser);

    fn unauthorized() -> Response {
        (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
    }

    fn internal<E: std::fmt::Display>(e: E) -> Response {
        eprintln!("rpc auth extractor error: {e}");
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
            let Some((token, _kind)) = auth_db::parse_session_token(authorization, cookie_header)
            else {
                return Err(unauthorized());
            };
            match auth_db::lookup_session(&pool, &token).await {
                Ok((user, _session)) => Ok(AuthUser {
                    id: user.id,
                    is_admin: user.is_admin,
                }),
                Err(AuthError::SessionNotFound) => Err(unauthorized()),
                Err(e) => Err(internal(e)),
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
    Ok(db::library_from_db(&pool.0, settings.ebook_library_path.as_deref()).await?)
}

/// FTS5-backed search across the configured ebook library. Empty or
/// whitespace-only `q` returns an empty library.
///
/// POST (not GET) so the query string can ride in the JSON body — Dioxus
/// `#[get]` server functions reject arg bodies because HTTP spec forbids
/// bodies on GET.
#[post("/api/rpc/search", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_search(q: String) -> Result<EbookLibrary> {
    let settings = db::get_settings(&pool.0).await?;
    let Some(path) = settings.ebook_library_path else {
        return Ok(EbookLibrary::default());
    };
    let books = db::search_books(&pool.0, &path, &q).await?;
    Ok(EbookLibrary {
        path: Some(path),
        books,
        error: None,
    })
}
