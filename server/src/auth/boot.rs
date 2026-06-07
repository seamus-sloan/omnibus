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
//!   parallel agents can rely on a known login. **Gated on
//!   `debug_assertions`** so release builds no-op even if the env var is
//!   somehow set in production — relying on operational discipline alone
//!   would be a footgun. The env var is sourced from a developer's `.env`
//!   (gitignored, sourced by `flake.nix`). Idempotent: an existing user
//!   is left untouched, so re-running boot won't reset a password the
//!   developer has rotated.
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
/// user (promoted to admin) if it doesn't already exist. No-op when:
///
/// - the binary is a release build (`!cfg!(debug_assertions)`) — hard
///   gate so a misconfigured prod env can't silently provision an admin;
/// - the env var is unset, malformed, or has empty halves;
/// - the user is already present (idempotent).
///
/// Log seed actions at `warn` so any stray seed event in prod logs stands
/// out (even though the release-build gate above should prevent them).
pub async fn seed_dev_user(pool: &SqlitePool) -> Result<(), omnibus_db::auth::AuthError> {
    // Release builds must not provision admin credentials from env, even
    // if OMNIBUS_DEV_SEED_USER is somehow set. We additionally warn so a
    // misconfigured deploy is visible in the logs.
    if !cfg!(debug_assertions) {
        if std::env::var_os("OMNIBUS_DEV_SEED_USER").is_some() {
            tracing::warn!(
                "OMNIBUS_DEV_SEED_USER is set in a release build; ignoring (dev-only hook)"
            );
        }
        return Ok(());
    }

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

    // create_user gates on registration_enabled, which flips to false
    // after the first user exists. Snapshot the current value, flip on
    // for the insert, then restore — so seeding doesn't accidentally
    // re-open registration in a half-bootstrapped DB.
    let prior_registration = omnibus_db::auth::registration_enabled(pool).await?;
    omnibus_db::auth::set_registration_enabled(pool, true).await?;
    let create_result = omnibus_db::auth::create_user(pool, username, password).await;
    // Restore the prior flag whether the insert succeeded or not. We
    // ignore restore errors so they don't mask a more useful create_user
    // error — surfacing the underlying failure matters more than a
    // best-effort restore.
    let _ = omnibus_db::auth::set_registration_enabled(pool, prior_registration).await;
    create_result?;

    let promoted = omnibus_db::auth::promote_to_admin(pool, username).await?;
    if !promoted {
        tracing::warn!(
            user = username,
            "OMNIBUS_DEV_SEED_USER created user but promote_to_admin did not update a row"
        );
    }
    tracing::warn!(
        user = username,
        "OMNIBUS_DEV_SEED_USER created admin user (dev seed hook)"
    );
    Ok(())
}

#[cfg(test)]
mod tests;
