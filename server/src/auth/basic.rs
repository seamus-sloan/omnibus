//! HTTP Basic authentication for the OPDS catalog (`OpdsAuthUser`).
//! Third-party OPDS clients (KOReader and friends) can only send
//! `Authorization: Basic`, so the `/opds/*` handlers accept it as a
//! fallback behind the normal cookie/bearer session path. Confined to
//! `/opds/*` — the `/api/*` surface stays cookie/bearer-only.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use axum::{
    extract::FromRequestParts,
    http::{header, request::Parts, StatusCode},
    response::{IntoResponse, Response},
};
use omnibus_db::auth::{self as auth_db, SessionKind};
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use tokio::sync::Mutex;

use crate::http_errors::internal;
use crate::rate_limit::{client_ip, RateLimiter};

use super::extractor::resolve_auth_user;
use super::AuthUser;

/// How long a verified `(username, password)` pair is trusted before the
/// next request pays Argon2 again. Bounds the window in which a changed
/// password keeps working, so keep it short.
const VERIFIED_TTL: Duration = Duration::from_secs(60);
/// Cap on cached credential entries before stale ones are pruned,
/// mirroring the rate limiter's bucket cap.
const MAX_VERIFIED_ENTRIES: usize = 10_000;

/// Shared state behind Basic verification: a per-IP limiter so credential
/// guessing is throttled, and a short-TTL cache of verified credentials so
/// a catalog page (which OPDS clients fetch link-by-link, replaying the
/// credentials every time) costs one Argon2 verify, not dozens.
///
/// Layered as an `Extension` by `opds_router` — the only router whose
/// handlers extract [`OpdsAuthUser`].
pub struct BasicAuthState {
    limiter: RateLimiter,
    verified: Mutex<HashMap<[u8; 32], VerifiedCred>>,
}

struct VerifiedCred {
    user_id: i64,
    verified_at: Instant,
}

impl BasicAuthState {
    /// Fresh state behind an `Arc`, ready to layer as an `Extension`.
    pub fn new_shared() -> Arc<Self> {
        Arc::new(Self {
            limiter: RateLimiter::new(),
            verified: Mutex::new(HashMap::new()),
        })
    }

    /// Look up a still-fresh cache entry for `key`, expiring it if stale.
    async fn cached_user_id(&self, key: &[u8; 32]) -> Option<i64> {
        let mut map = self.verified.lock().await;
        if map.len() >= MAX_VERIFIED_ENTRIES {
            map.retain(|_, c| c.verified_at.elapsed() < VERIFIED_TTL);
        }
        match map.get(key) {
            Some(c) if c.verified_at.elapsed() < VERIFIED_TTL => Some(c.user_id),
            Some(_) => {
                map.remove(key);
                None
            }
            None => None,
        }
    }

    async fn store(&self, key: [u8; 32], user_id: i64) {
        self.verified.lock().await.insert(
            key,
            VerifiedCred {
                user_id,
                verified_at: Instant::now(),
            },
        );
    }
}

/// Cache key: SHA-256 over the credentials, never the plaintext pair.
/// The username is lowercased to match the `COLLATE NOCASE` lookup in
/// `verify_login`; the password participates verbatim.
fn cred_cache_key(username: &str, password: &str) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(username.to_lowercase().as_bytes());
    hasher.update([0u8]);
    hasher.update(password.as_bytes());
    hasher.finalize().into()
}

/// `401` carrying a `WWW-Authenticate: Basic` challenge, so OPDS clients
/// (and browsers) know to send credentials. The plain [`AuthUser`] 401
/// deliberately has no challenge — only the OPDS surface speaks Basic.
fn unauthorized_with_challenge() -> Response {
    (
        StatusCode::UNAUTHORIZED,
        [(
            header::WWW_AUTHENTICATE,
            "Basic realm=\"omnibus\", charset=\"UTF-8\"",
        )],
        "unauthorized",
    )
        .into_response()
}

/// Authenticated user for the `/opds/*` catalog: the normal cookie/bearer
/// session path first, then HTTP Basic credentials as a fallback.
#[derive(Debug, Clone)]
pub struct OpdsAuthUser(pub AuthUser);

impl<S> FromRequestParts<S> for OpdsAuthUser
where
    S: Send + Sync,
{
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        match resolve_auth_user(parts, false).await {
            Ok(user) => return Ok(Self(user)),
            // A session-path 401 falls through to Basic; anything else
            // (e.g. a 500 from the session lookup) is already the answer.
            Err(resp) if resp.status() != StatusCode::UNAUTHORIZED => return Err(resp),
            Err(_) => {}
        }
        let creds = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(auth_db::parse_basic_credentials);
        let Some((username, password)) = creds else {
            return Err(unauthorized_with_challenge());
        };
        let pool = parts
            .extensions
            .get::<SqlitePool>()
            .cloned()
            .ok_or_else(|| internal("opds basic auth", "missing SqlitePool extension"))?;
        let basic = parts
            .extensions
            .get::<Arc<BasicAuthState>>()
            .cloned()
            .ok_or_else(|| internal("opds basic auth", "missing BasicAuthState extension"))?;

        let key = cred_cache_key(&username, &password);
        if let Some(user_id) = basic.cached_user_id(&key).await {
            // Re-read the row so permission edits apply within the TTL;
            // only the Argon2 verify is cached, not the user.
            return match auth_db::get_user_by_id(&pool, user_id).await {
                Ok(Some(user)) => Ok(Self(basic_auth_user(user))),
                Ok(None) => Err(unauthorized_with_challenge()),
                Err(e) => Err(internal("opds basic auth", e)),
            };
        }

        // Uncached credentials pay the limiter before Argon2, so an
        // attacker can't burn CPU (or brute-force) faster than the
        // login endpoints allow.
        let ip = client_ip(&parts.extensions, &parts.headers);
        if !basic.limiter.allow(ip).await {
            return Err((StatusCode::TOO_MANY_REQUESTS, "rate limit exceeded").into_response());
        }
        match auth_db::verify_login(&pool, &username, &password).await {
            Ok(user) => {
                basic.store(key, user.id).await;
                Ok(Self(basic_auth_user(user)))
            }
            Err(auth_db::AuthError::InvalidCredentials) => Err(unauthorized_with_challenge()),
            Err(auth_db::AuthError::AccountLocked { .. }) => {
                Err((StatusCode::FORBIDDEN, "account temporarily locked").into_response())
            }
            Err(e) => Err(internal("opds basic auth", e)),
        }
    }
}

/// Assemble an [`AuthUser`] for a Basic-authenticated request. No session
/// row backs it — `session_id` is a `0` sentinel and the kind is
/// [`SessionKind::Basic`], neither of which any handler on the OPDS
/// surface reads.
fn basic_auth_user(user: auth_db::User) -> AuthUser {
    AuthUser {
        id: user.id,
        username: user.username,
        is_admin: user.is_admin,
        can_upload: user.can_upload,
        can_edit: user.can_edit,
        can_download: user.can_download,
        kindle_email: user.kindle_email,
        display_name: user.display_name,
        has_avatar: user.has_avatar,
        hidden_formats: user.hidden_formats,
        session_id: 0,
        session_kind: SessionKind::Basic,
    }
}
