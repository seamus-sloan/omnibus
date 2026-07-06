//! Server functions (`/api/rpc/*`) callable from the web client — mobile
//! instead uses the hand-written `/api/*` REST routes (`server/src/backend.rs`).
//! Per-domain submodules hold the functions and are re-exported here so
//! callsites keep importing `crate::rpc::rpc_*`; this module owns only the
//! shared `PoolExt` / `WorkerExt` / `AdminUser` / `AuthUser` pieces.

mod account;
mod authors;
mod bookmarks;
mod books;
mod highlights;
mod journals;
mod kindle;
mod overrides;
mod palette;
mod progress;
mod ratings;
mod series;
mod settings;
mod shelves;

pub use account::*;
pub use authors::*;
pub use bookmarks::*;
pub use books::*;
pub use highlights::*;
pub use journals::*;
pub use kindle::*;
pub use overrides::*;
pub use palette::*;
pub use progress::*;
pub use ratings::*;
pub use series::*;
pub use settings::*;
pub use shelves::*;

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

/// Log an internal RPC failure server-side and return a generic client-facing
/// `ServerFnError`, so raw `sqlx`/transport error text (which can carry SQL,
/// schema, or network details) never reaches the client — mirrors the REST
/// `server::backend::internal` helper for the `/api/rpc/*` surface. Returns
/// the concrete `ServerFnError` (rather than a generic `Into`-bound `Result`
/// error) so it composes with both `Err(internal_error(...).into())` match
/// arms and `.map_err(|e| internal_error(...))?` chains without the compiler
/// needing to disambiguate two different `From` conversions at once.
#[cfg(feature = "server")]
pub(crate) fn internal_error<E>(op: &'static str, e: E) -> dioxus::fullstack::ServerFnError
where
    E: std::fmt::Display,
{
    tracing::error!(op = op, error = %e, "rpc internal error");
    dioxus::fullstack::ServerFnError::new("internal server error")
}

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
