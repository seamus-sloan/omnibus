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
//! * [`rate_limit`] — per-IP fixed-window counter + `rate_limit_auth` middleware
//!   scoped to the login/register endpoints.
//! * [`strategy`] — `AuthStrategy` trait + `PasswordStrategy` impl. OIDC
//!   and WebAuthn fit the same shape.
//! * [`boot`] — `OMNIBUS_INITIAL_ADMIN` recovery hook.
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
pub mod rate_limit;
pub mod strategy;

#[cfg(test)]
pub mod test_support;

pub use csrf::origin_check;
pub use extractor::{AdminUser, AuthUser};
pub use gate::require_auth;
pub use handlers::auth_router;
pub use rate_limit::{rate_limit_auth, RateLimiter};

/// Name of the session cookie issued to web clients. Not using the
/// `__Host-` prefix so the dev server on plain HTTP still works; production
/// deployments behind HTTPS should set `OMNIBUS_SECURE_COOKIES=1` to toggle
/// the `Secure` attribute.
pub const SESSION_COOKIE: &str = "omnibus_session";

/// 30 days for cookie sessions; matches the plan's absolute expiry.
pub const COOKIE_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// 90 days for mobile bearer tokens.
pub const BEARER_TTL_SECS: i64 = 90 * 24 * 60 * 60;
