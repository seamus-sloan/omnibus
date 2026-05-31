//! Server-side axum glue on top of [`omnibus_db::auth`]: cookie/bearer
//! extractors ([`extractor`]), `/api/auth/{register,login,logout,me}` plus
//! [`auth_router`] ([`handlers`]), CSRF origin check ([`csrf`]), pluggable
//! auth backends ([`strategy`]), and the `OMNIBUS_INITIAL_ADMIN` recovery
//! hook ([`boot`]). Per-route extractors (`AuthUser` / `AdminUser`) are
//! what enforce permissions; [`gate::require_auth`] is just the boundary.

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

/// Plain session-cookie name used on plain-HTTP dev origins. Re-exported
/// from `omnibus_db::auth::SESSION_COOKIE_NAME` so cookie issuance
/// (`Set-Cookie`), CSRF cookie checks, and token parsing all share a
/// single source of truth.
pub use omnibus_db::auth::SESSION_COOKIE_NAME as SESSION_COOKIE;

/// `__Host-` prefixed session-cookie name (RFC 6265bis) used when
/// `OMNIBUS_SECURE_COOKIES` is on (the default). The prefix is a
/// browser-enforced contract — `Secure` + no `Domain` + `Path=/` — and
/// blocks subdomain cookie injection from a sibling host. The write side
/// in [`handlers::session_cookie`] is the single place that picks between
/// this and the plain form via [`session_cookie_name`]; the read side
/// (token parser, CSRF jar lookup) accepts either so a deploy that
/// toggles `OMNIBUS_SECURE_COOKIES` mid-rollout doesn't invalidate
/// in-flight cookies.
pub use omnibus_db::auth::SESSION_COOKIE_NAME_HOST_PREFIXED as SESSION_COOKIE_HOST_PREFIXED;

/// Pure selector used by the cookie write path: returns the `__Host-`
/// prefixed name when `secure` is true (production HTTPS), the plain
/// name otherwise. Kept pure so it can be unit-tested without env or
/// `OnceLock` state.
pub fn session_cookie_name(secure: bool) -> &'static str {
    if secure {
        SESSION_COOKIE_HOST_PREFIXED
    } else {
        SESSION_COOKIE
    }
}

/// 30 days for cookie sessions; matches the plan's absolute expiry.
pub const COOKIE_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// 90 days for mobile bearer tokens.
pub const BEARER_TTL_SECS: i64 = 90 * 24 * 60 * 60;
