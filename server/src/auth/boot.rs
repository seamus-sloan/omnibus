//! `OMNIBUS_INITIAL_ADMIN` recovery hook.
//!
//! On boot, if the env var is set, promote the named existing user to admin.
//! This is explicitly a recovery escape hatch ("I locked myself out") and
//! *not* a provisioning mechanism — we don't auto-create a user from an env
//! var since that would require smuggling a password in.
//!
//! Successful promotions are logged at `warn` so they show up in the audit
//! trail; setting the env var for a user that doesn't exist also logs at
//! `warn` so the misconfiguration is visible.

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
}
