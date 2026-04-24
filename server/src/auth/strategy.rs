//! `AuthStrategy` — pluggable authentication back-ends.
//!
//! v1.0 ships the `PasswordStrategy` (username + argon2id PHC), but F5.x
//! adds OIDC, and post-v1.0 could bring passkeys/WebAuthn. This trait
//! exists now so those extensions drop in without reshaping the login flow.
//!
//! Design notes:
//! - The trait returns a `UserId`, not `(username, password)` — keeping it
//!   credential-agnostic is what makes WebAuthn/OIDC fit later.
//! - `kind()` is a short stable tag (`"password"`, `"oidc"`, `"webauthn"`)
//!   surfaced to the admin UI ("how did this user last log in?") and used
//!   to gate strategy-specific settings.
//! - Concrete `OidcStrategy` and `WebAuthnStrategy` are deferred. The trait
//!   only needs to prove it's shaped right; implementations can land in
//!   their own phases.

use async_trait::async_trait;
use omnibus_db::auth::AuthError;
use sqlx::SqlitePool;

/// Opaque reference to a row in `users`.
pub type UserId = i64;

/// A verified authentication attempt. Contains enough for the handler to
/// issue a session; no password or secret material.
#[derive(Debug, Clone)]
pub struct AuthenticatedUser {
    pub user_id: UserId,
}

/// Contract for a pluggable auth backend. Implementations do the crypto +
/// DB lookup; the session-issuing + cookie/bearer plumbing stays in the
/// handler layer so every strategy lands sessions the same way.
#[async_trait]
pub trait AuthStrategy: Send + Sync {
    /// Short stable tag for logs and admin UI (`"password"`, `"oidc"`, …).
    fn kind(&self) -> &'static str;

    /// Authenticate a `(username, secret)` pair and resolve it to a user row.
    /// Password strategies will use `secret` as the password; OIDC/WebAuthn
    /// implementations will ignore it in favor of their own flow and accept
    /// the trait shape as-is, probably exposing additional methods for the
    /// redirect/assertion dance.
    async fn authenticate(
        &self,
        pool: &SqlitePool,
        username: &str,
        secret: &str,
    ) -> Result<AuthenticatedUser, AuthError>;
}

/// Username + Argon2id PHC password strategy. Thin wrapper over
/// [`omnibus_db::auth::verify_login`] so the hashing/lockout/timing-equal
/// logic stays in one place.
pub struct PasswordStrategy;

#[async_trait]
impl AuthStrategy for PasswordStrategy {
    fn kind(&self) -> &'static str {
        "password"
    }

    async fn authenticate(
        &self,
        pool: &SqlitePool,
        username: &str,
        secret: &str,
    ) -> Result<AuthenticatedUser, AuthError> {
        let user = omnibus_db::auth::verify_login(pool, username, secret).await?;
        Ok(AuthenticatedUser { user_id: user.id })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use omnibus_db as db;

    #[tokio::test]
    async fn password_strategy_roundtrip() {
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        db::auth::create_user(&pool, "alice", "correct horse battery staple")
            .await
            .unwrap();
        let s = PasswordStrategy;
        assert_eq!(s.kind(), "password");
        let a = s
            .authenticate(&pool, "alice", "correct horse battery staple")
            .await
            .unwrap();
        assert!(a.user_id > 0);
    }

    #[tokio::test]
    async fn password_strategy_rejects_bad_password() {
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        db::auth::create_user(&pool, "alice", "correct horse battery staple")
            .await
            .unwrap();
        let s = PasswordStrategy;
        let r = s.authenticate(&pool, "alice", "nope").await;
        assert!(matches!(r, Err(AuthError::InvalidCredentials)));
    }
}
