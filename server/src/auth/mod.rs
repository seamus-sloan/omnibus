//! F0.3 auth — server-side axum glue on top of [`omnibus_db::auth`].
//!
//! Layout:
//!
//! * [`extractor`] — `AuthUser` / `AdminUser` `FromRequestParts` extractors
//!   that resolve a live session from either the `omnibus_session` cookie
//!   or an `Authorization: Bearer` header.
//! * [`handlers`] — `/api/auth/{register,login,logout,me}` + [`auth_router`].
//! * [`csrf`] — `origin_check` middleware for cookie-authed state-changing
//!   requests.
//! * [`strategy`] — `AuthStrategy` trait + `PasswordStrategy` impl. OIDC
//!   and WebAuthn fit the same shape.
//! * [`boot`] — `OMNIBUS_INITIAL_ADMIN` recovery hook.
//!
//! Per-IP rate limiting lives in the top-level [`crate::rate_limit`] module
//! since it is shared by the auth endpoints and the search endpoints. The
//! auth router mounts it via `rate_limit_by_ip` in `main.rs`.
//!
//! Per-route enforcement (F0.7): every protected handler in
//! [`crate::backend`] and every server function in `omnibus_frontend::rpc`
//! declares the strictest extractor it needs (`AuthUser` for read paths,
//! `AdminUser` for state-changing ops on shared config). The middleware
//! [`gate::require_auth`] is just the boundary; the per-route extractors
//! are what actually enforce the permission columns.

pub mod boot;
pub mod csrf;
pub mod extractor;
pub mod gate;
pub mod handlers;
pub mod strategy;

#[cfg(test)]
pub mod test_support;

pub use csrf::origin_check;
pub use extractor::{AdminUser, AuthUser};
pub use gate::require_auth;
pub use handlers::auth_router;

/// Name of the session cookie issued to web clients. Re-exported from
/// `omnibus_db::auth::SESSION_COOKIE_NAME` so cookie issuance
/// (`Set-Cookie`), CSRF cookie checks, and token parsing all share a
/// single source of truth. Not using the `__Host-` prefix so the dev
/// server on plain HTTP still works; production deployments behind HTTPS
/// should set `OMNIBUS_SECURE_COOKIES=1` to toggle the `Secure` attribute.
pub use omnibus_db::auth::SESSION_COOKIE_NAME as SESSION_COOKIE;

/// 30 days for cookie sessions; matches the plan's absolute expiry.
pub const COOKIE_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// 90 days for mobile bearer tokens.
pub const BEARER_TTL_SECS: i64 = 90 * 24 * 60 * 60;
