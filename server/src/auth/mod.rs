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
//! No existing `/api/*` routes are gated yet — PR3 flips that switch.

pub mod boot;
pub mod csrf;
pub mod extractor;
pub mod handlers;
pub mod rate_limit;
pub mod strategy;

pub use csrf::origin_check;
pub use extractor::{AdminUser, AuthUser};
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
