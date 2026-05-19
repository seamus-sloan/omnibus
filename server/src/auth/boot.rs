//! Boot-time admin hooks.
//!
//! Two opt-in env-driven hooks run at server startup:
//!
//! * [`apply_initial_admin`] — `OMNIBUS_INITIAL_ADMIN=<username>` promotes
//!   an existing user to admin. Recovery escape hatch ("I locked myself
//!   out"); never creates a user.
//! * [`seed_dev_user`] — `OMNIBUS_DEV_SEED_USER=<username>:<password>`
//!   creates the named user (and promotes to admin) if it doesn't exist
//!   yet. Strictly a dev convenience so Claude's `ui-validate` skill and
//!   parallel agents can rely on a known login. The env var is only set
//!   from a developer's `.env` (gitignored, sourced by `flake.nix`) —
//!   production deployments must never set it. Idempotent: an existing
//!   user is left untouched, so re-running boot won't reset a password
//!   the developer has rotated.
//!
//! Successful promotions and seeds are logged at `warn` so they show up
//! in the audit trail; misconfigurations also log at `warn` so they're
//! visible.

use sqlx::SqlitePool;

/// Read `OMNIBUS_INITIAL_ADMIN` and promote the named user (if present) to
/// admin. No-op when the env var is unset or the named user doesn't exist.
pub async fn apply_initial_admin(pool: &SqlitePool) -> Result<(), omnibus_db::auth::AuthError> {
    let Ok(username) = std::env::var("OMNIBUS_INITIAL_ADMIN") else {
        return Ok(());
    };
    let username = username.trim();
    if username.is_empty() {
        return Ok(());
    }
    let promoted = omnibus_db::auth::promote_to_admin(pool, username).await?;
    if promoted {
        tracing::warn!(
            user = username,
            "OMNIBUS_INITIAL_ADMIN promoted user to admin (recovery hook)"
        );
    } else {
        tracing::warn!(
            user = username,
            "OMNIBUS_INITIAL_ADMIN set but user does not exist; no promotion performed"
        );
    }
    Ok(())
}

/// Read `OMNIBUS_DEV_SEED_USER=<username>:<password>` and create the named
/// user (promoted to admin) if it doesn't already exist. No-op when the
/// env var is unset, malformed, or the user is already present.
///
/// Always log seed actions at `warn` — this hook should never fire in
/// production, so a stray seed event in prod logs is meant to stand out.
pub async fn seed_dev_user(pool: &SqlitePool) -> Result<(), omnibus_db::auth::AuthError> {
    let Ok(raw) = std::env::var("OMNIBUS_DEV_SEED_USER") else {
        return Ok(());
    };
    let raw = raw.trim();
    if raw.is_empty() {
        return Ok(());
    }
    let Some((username, password)) = raw.split_once(':') else {
        tracing::warn!("OMNIBUS_DEV_SEED_USER is malformed; expected username:password, ignoring");
        return Ok(());
    };
    let username = username.trim();
    if username.is_empty() || password.is_empty() {
        tracing::warn!("OMNIBUS_DEV_SEED_USER has an empty username or password; ignoring");
        return Ok(());
    }

    if omnibus_db::auth::get_user_by_username(pool, username)
        .await?
        .is_some()
    {
        tracing::debug!(
            user = username,
            "OMNIBUS_DEV_SEED_USER already exists; no-op"
        );
        return Ok(());
    }

    // Registration may have been disabled (set_registration_enabled flips
    // to false once the first user exists). create_user gates on that
    // flag for non-first users, so re-enable around the insert. If the
    // table is empty, the first-user path engages and admin is set
    // automatically; otherwise we promote afterwards.
    omnibus_db::auth::set_registration_enabled(pool, true).await?;
    omnibus_db::auth::create_user(pool, username, password).await?;
    let _ = omnibus_db::auth::promote_to_admin(pool, username).await?;
    tracing::warn!(
        user = username,
        "OMNIBUS_DEV_SEED_USER created admin user (dev seed hook)"
    );
    Ok(())
}

#[cfg(test)]
// Each test in this module holds `ENV_LOCK` across `await` points on
// purpose — `std::env::{set_var, remove_var}` is process-global and racy,
// so we serialize via a `std::sync::Mutex` (an async mutex wouldn't help
// because the lock guards the env var itself, not just coroutine turns).
#[allow(clippy::await_holding_lock)]
mod tests {
    use super::*;
    use omnibus_db as db;

    // std::env is global so these tests can't be parallelised safely; we
    // serialize on a static mutex. Also restore the prior value on drop.
    use std::sync::Mutex;
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    struct EnvGuard {
        key: &'static str,
        prev: Option<String>,
    }
    impl EnvGuard {
        fn set(key: &'static str, value: &str) -> Self {
            let prev = std::env::var(key).ok();
            // Safety: we hold ENV_LOCK for the duration of any test using this.
            unsafe { std::env::set_var(key, value) };
            Self { key, prev }
        }
        fn unset(key: &'static str) -> Self {
            let prev = std::env::var(key).ok();
            unsafe { std::env::remove_var(key) };
            Self { key, prev }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            unsafe {
                match &self.prev {
                    Some(v) => std::env::set_var(self.key, v),
                    None => std::env::remove_var(self.key),
                }
            }
        }
    }

    #[tokio::test]
    async fn unset_env_is_noop() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::unset("OMNIBUS_INITIAL_ADMIN");
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        db::auth::create_user(&pool, "alice", "correct horse battery staple")
            .await
            .unwrap();
        apply_initial_admin(&pool).await.unwrap();
        // alice is the first user, so she's already admin — unchanged.
        let u = db::auth::get_user_by_username(&pool, "alice")
            .await
            .unwrap()
            .unwrap();
        assert!(u.is_admin);
    }

    #[tokio::test]
    async fn env_promotes_existing_non_admin() {
        let _lock = ENV_LOCK.lock().unwrap();
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        // alice becomes admin (first user). bob does not.
        db::auth::create_user(&pool, "alice", "correct horse battery staple")
            .await
            .unwrap();
        db::auth::set_registration_enabled(&pool, true)
            .await
            .unwrap();
        db::auth::create_user(&pool, "bob", "correct horse battery staple")
            .await
            .unwrap();
        let bob = db::auth::get_user_by_username(&pool, "bob")
            .await
            .unwrap()
            .unwrap();
        assert!(!bob.is_admin);

        let _g = EnvGuard::set("OMNIBUS_INITIAL_ADMIN", "bob");
        apply_initial_admin(&pool).await.unwrap();

        let bob = db::auth::get_user_by_username(&pool, "bob")
            .await
            .unwrap()
            .unwrap();
        assert!(bob.is_admin);
    }

    #[tokio::test]
    async fn env_for_unknown_user_is_noop_no_error() {
        let _lock = ENV_LOCK.lock().unwrap();
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        let _g = EnvGuard::set("OMNIBUS_INITIAL_ADMIN", "ghost");
        apply_initial_admin(&pool).await.unwrap();
    }

    // ----- seed_dev_user -----

    #[tokio::test]
    async fn seed_unset_env_is_noop() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::unset("OMNIBUS_DEV_SEED_USER");
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        seed_dev_user(&pool).await.unwrap();
        assert!(db::auth::get_user_by_username(&pool, "admin")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn seed_creates_admin_when_user_absent() {
        let _lock = ENV_LOCK.lock().unwrap();
        let _g = EnvGuard::set("OMNIBUS_DEV_SEED_USER", "admin:omnibus-dev");
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        seed_dev_user(&pool).await.unwrap();
        let u = db::auth::get_user_by_username(&pool, "admin")
            .await
            .unwrap()
            .expect("admin should exist");
        assert!(u.is_admin, "seeded user should be admin");
    }

    #[tokio::test]
    async fn seed_is_idempotent_when_user_exists() {
        let _lock = ENV_LOCK.lock().unwrap();
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        // Pre-create admin with a known password.
        db::auth::create_user(&pool, "admin", "preexisting-password")
            .await
            .unwrap();

        let _g = EnvGuard::set("OMNIBUS_DEV_SEED_USER", "admin:different-password");
        seed_dev_user(&pool).await.unwrap();

        // Login with the *original* password should still work — the
        // hook did not overwrite it.
        db::auth::verify_login(&pool, "admin", "preexisting-password")
            .await
            .expect("original password should still authenticate");
    }

    #[tokio::test]
    async fn seed_malformed_env_is_noop_no_error() {
        let _lock = ENV_LOCK.lock().unwrap();
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        // Missing ':' delimiter.
        let _g = EnvGuard::set("OMNIBUS_DEV_SEED_USER", "no-colon-here");
        seed_dev_user(&pool).await.unwrap();
        assert!(db::auth::get_user_by_username(&pool, "no-colon-here")
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn seed_works_when_other_users_already_exist() {
        let _lock = ENV_LOCK.lock().unwrap();
        let pool = db::init_db("sqlite::memory:").await.unwrap();
        // alice is the first user (admin); registration closes afterwards.
        db::auth::create_user(&pool, "alice", "correct horse battery staple")
            .await
            .unwrap();

        let _g = EnvGuard::set("OMNIBUS_DEV_SEED_USER", "admin:omnibus-dev");
        seed_dev_user(&pool).await.unwrap();

        let u = db::auth::get_user_by_username(&pool, "admin")
            .await
            .unwrap()
            .expect("admin should be created even after registration closed");
        assert!(u.is_admin);
    }
}
