//! Tests for the OMNIBUS_INITIAL_ADMIN recovery hook.
use super::*;
use omnibus_db as db;

// std::env is global so these tests can't be parallelised safely; we
// serialize on a static mutex. `EnvGuard` acquires this lock itself in
// its constructor and holds it for its entire lifetime — callers cannot
// construct an `EnvGuard` without implicitly holding the lock, so the
// safety invariant is enforced by construction rather than by convention.
use std::sync::{Mutex, MutexGuard};
static ENV_LOCK: Mutex<()> = Mutex::new(());

/// RAII guard that serializes access to `std::env` and restores the
/// prior value on drop.
///
/// `EnvGuard::set` / `EnvGuard::unset` acquire `ENV_LOCK` themselves and
/// store the `MutexGuard` inside the struct.  The guard is released when
/// the `EnvGuard` is dropped (after the env var is restored), so it is
/// structurally impossible to mutate the environment without holding the
/// lock — no documentation contract for callers to misunderstand.
///
/// The `MutexGuard` is held across `.await` points in the tests that use
/// this helper, which clippy flags.  The `#[allow]` is on the individual
/// tests that do so (rather than the whole module) so it is scoped as
/// tightly as possible.  An async mutex is not appropriate here: the lock
/// guards the process-global environment, not just a single coroutine
/// turn, so we must use a `std::sync::Mutex` and intentionally hold it
/// across yield points to prevent other tests from racing on env.
struct EnvGuard {
    key: &'static str,
    prev: Option<String>,
    // Held for the lifetime of this guard; released on drop after env is
    // restored.  Named with a leading underscore to communicate "held for
    // side-effect" to the compiler (suppresses unused-variable lint).
    _lock: MutexGuard<'static, ()>,
}

impl EnvGuard {
    fn set(key: &'static str, value: &str) -> Self {
        // Recover from a poisoned lock (a prior test panicked while
        // holding it) instead of cascading the panic into every
        // subsequent env-var test — matches the repo's other ENV_LOCK
        // / mutex call sites (e.g. db::settings, db::worker).
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var(key).ok();
        // SAFETY: `_lock` guarantees exclusive access to the process
        // environment for the duration of this guard's lifetime —
        // acquired above and held until `Drop` restores the prior value
        // and then releases the lock.  No concurrent env mutation can
        // occur while the guard is alive.
        unsafe { std::env::set_var(key, value) };
        Self {
            key,
            prev,
            _lock: lock,
        }
    }

    fn unset(key: &'static str) -> Self {
        // Recover from a poisoned lock (a prior test panicked while
        // holding it) instead of cascading the panic into every
        // subsequent env-var test — matches the repo's other ENV_LOCK
        // / mutex call sites (e.g. db::settings, db::worker).
        let lock = ENV_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        let prev = std::env::var(key).ok();
        // SAFETY: same guarantee as `EnvGuard::set` — `_lock` is held
        // for the entire guard lifetime, preventing concurrent env access.
        unsafe { std::env::remove_var(key) };
        Self {
            key,
            prev,
            _lock: lock,
        }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: `self._lock` is still live — `MutexGuard` fields are
        // dropped after other fields in declaration order, but even if
        // reordering occurred the lock is still held at this point
        // because `_lock` and the env mutation are part of the same
        // struct.  Regardless, `_lock` is the last meaningful field, so
        // we restore the environment while the lock is unambiguously held.
        unsafe {
            match &self.prev {
                Some(v) => std::env::set_var(self.key, v),
                None => std::env::remove_var(self.key),
            }
        }
        // `self._lock` is dropped here (end of `drop`), releasing the mutex.
    }
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn unset_env_is_noop() {
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

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn env_promotes_existing_non_admin() {
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

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn env_for_unknown_user_is_noop_no_error() {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    let _g = EnvGuard::set("OMNIBUS_INITIAL_ADMIN", "ghost");
    apply_initial_admin(&pool).await.unwrap();
}

// ----- seed_dev_user -----

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn seed_unset_env_is_noop() {
    let _g = EnvGuard::unset("OMNIBUS_DEV_SEED_USER");
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    seed_dev_user(&pool).await.unwrap();
    assert!(db::auth::get_user_by_username(&pool, "admin")
        .await
        .unwrap()
        .is_none());
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn seed_creates_admin_when_user_absent() {
    let _g = EnvGuard::set("OMNIBUS_DEV_SEED_USER", "admin:omnibus-dev");
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    seed_dev_user(&pool).await.unwrap();
    let u = db::auth::get_user_by_username(&pool, "admin")
        .await
        .unwrap()
        .expect("admin should exist");
    assert!(u.is_admin, "seeded user should be admin");
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn seed_is_idempotent_when_user_exists() {
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

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn seed_malformed_env_is_noop_no_error() {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    // Missing ':' delimiter.
    let _g = EnvGuard::set("OMNIBUS_DEV_SEED_USER", "no-colon-here");
    seed_dev_user(&pool).await.unwrap();
    assert!(db::auth::get_user_by_username(&pool, "no-colon-here")
        .await
        .unwrap()
        .is_none());
}

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn seed_works_when_other_users_already_exist() {
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

#[allow(clippy::await_holding_lock)]
#[tokio::test]
async fn seed_restores_prior_registration_enabled_flag() {
    // Reviewer PR #71 / boot.rs:94 — seed_dev_user used to leave
    // registration_enabled=true after running, accidentally re-opening
    // public registration in a half-bootstrapped DB. The fix snapshots
    // the flag before the insert and restores it after.
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    // First user closes registration as a side effect.
    db::auth::create_user(&pool, "alice", "correct horse battery staple")
        .await
        .unwrap();
    assert!(!db::auth::registration_enabled(&pool).await.unwrap());

    let _g = EnvGuard::set("OMNIBUS_DEV_SEED_USER", "admin:omnibus-dev");
    seed_dev_user(&pool).await.unwrap();

    assert!(
        !db::auth::registration_enabled(&pool).await.unwrap(),
        "seed_dev_user must restore the prior registration_enabled flag"
    );
}
