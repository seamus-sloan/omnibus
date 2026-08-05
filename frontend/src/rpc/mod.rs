//! Server functions (`/api/rpc/*`) callable from the web client — mobile
//! instead uses the hand-written `/api/*` REST routes (`server/src/backend.rs`).
//! Per-domain submodules hold the functions and are re-exported here so
//! callsites keep importing `crate::rpc::rpc_*`; this module owns only the
//! shared `PoolExt` / `WorkerExt` / `AdminUser` / `AuthUser` pieces.

#[cfg(feature = "server")]
use dioxus::prelude::ServerFnError;

#[cfg(all(test, feature = "server"))]
mod tests;

mod account;
mod authors;
mod bookmarks;
mod books;
mod hardcover_fetch;
mod highlights;
mod journals;
mod kindle;
mod kobo;
mod logs;
mod overrides;
mod palette;
mod physical;
mod progress;
mod ratings;
mod read_status;
mod scan;
mod series;
mod settings;
mod shelves;
mod stats;
mod summary;

pub use account::*;
pub use authors::*;
pub use bookmarks::*;
pub use books::*;
pub use hardcover_fetch::*;
pub use highlights::*;
pub use journals::*;
pub use kindle::*;
pub use kobo::*;
pub use logs::*;
pub use overrides::*;
pub use palette::*;
pub use physical::*;
pub use progress::*;
pub use ratings::*;
pub use read_status::*;
pub use scan::*;
pub use series::*;
pub use settings::*;
pub use shelves::*;
pub use stats::*;
pub use summary::*;

/// Log the real error server-side and return a generic message safe to hand
/// to the client, mirroring `server/src/backend.rs`'s `internal()` one layer
/// up (HTTP/RPC response boundary instead of a Rust module boundary — see
/// `.claude/rules/02-error-handling.md`'s boundary rule). Only for opaque
/// failures (`Sqlx`, transport, IO) — typed/validation variants a client
/// legitimately branches on should keep their specific message.
#[cfg(feature = "server")]
pub(crate) fn internal_rpc_error<E: std::fmt::Display>(
    context: &'static str,
    e: E,
) -> ServerFnError {
    tracing::error!(error = %e, context = context, "rpc handler error");
    ServerFnError::new("internal server error")
}

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
/// `#[post]` macros in the submodules. These are deliberately scoped to this
/// module instead of imported from `crate::omnibus::auth` — the `frontend`
/// crate can't depend on the `server` crate (cycle), and dioxus already
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
        /// Metadata edit permission. `true` for the first user (admin) and
        /// any user explicitly granted `can_edit`.
        pub can_edit: bool,
        /// The live session this request authenticated with. Threaded into
        /// `db::auth::change_password` so a self-service password change can
        /// exclude the caller's own session from its revocation sweep (#1402).
        pub session_id: i64,
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
                Ok((user, session)) => Ok(AuthUser {
                    id: user.id,
                    is_admin: user.is_admin,
                    can_edit: user.can_edit,
                    session_id: session.id,
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
