//! The admin-overridable `ebook_convert_path` setting. One KV row with the
//! seed-from-env-then-settings-wins precedence the provider keys and SMTP
//! config use; resolution order for the effective command is settings, then
//! `$OMNIBUS_EBOOK_CONVERT_PATH`, then the bare `ebook-convert` name on
//! `$PATH`.

use omnibus_shared::{EbookConvertStatus, DEFAULT_EBOOK_CONVERT_BIN, PATH_MAX_LEN};
use sqlx::SqlitePool;

use crate::convert;

use super::{non_empty_env, SettingsError};

/// `settings` KV key for the Calibre `ebook-convert` path override. Stored
/// directly (not via the [`super::Settings`] struct) so saving it never
/// triggers the scan-root reconciliation `set_settings` runs.
const EBOOK_CONVERT_PATH_KEY: &str = "ebook_convert_path";

/// Env var consulted when no override is saved.
const EBOOK_CONVERT_PATH_ENV: &str = "OMNIBUS_EBOOK_CONVERT_PATH";

/// Read the saved `ebook_convert_path` override, or `None` when unset/blank.
pub async fn get_ebook_convert_path(pool: &SqlitePool) -> Result<Option<String>, SettingsError> {
    let v = sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
        .bind(EBOOK_CONVERT_PATH_KEY)
        .fetch_optional(pool)
        .await?;
    Ok(v.map(|s| s.trim().to_string()).filter(|s| !s.is_empty()))
}

/// Persist (or clear, when `None`/blank) the `ebook_convert_path` override.
/// Rejects a path longer than [`PATH_MAX_LEN`] with
/// [`SettingsError::Validation`] before the write.
pub async fn set_ebook_convert_path(
    pool: &SqlitePool,
    path: Option<&str>,
) -> Result<(), SettingsError> {
    match path.map(str::trim).filter(|s| !s.is_empty()) {
        Some(p) if p.len() > PATH_MAX_LEN => Err(SettingsError::Validation(format!(
            "ebook convert path exceeds {PATH_MAX_LEN} bytes"
        ))),
        Some(p) => {
            sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)")
                .bind(EBOOK_CONVERT_PATH_KEY)
                .bind(p)
                .execute(pool)
                .await?;
            Ok(())
        }
        None => {
            sqlx::query("DELETE FROM settings WHERE key = ?")
                .bind(EBOOK_CONVERT_PATH_KEY)
                .execute(pool)
                .await?;
            Ok(())
        }
    }
}

/// The command the conversion path should invoke, and where it came from: the
/// saved override wins (`"settings"`), then the env var (`"env"`), then the
/// bare `ebook-convert` name resolved on `$PATH` (`"path"`). Never `None` —
/// whether the command actually runs is [`ebook_convert_status`]'s question.
pub async fn effective_ebook_convert_path(
    pool: &SqlitePool,
) -> Result<(String, &'static str), SettingsError> {
    if let Some(saved) = get_ebook_convert_path(pool).await? {
        return Ok((saved, "settings"));
    }
    if let Some(from_env) = non_empty_env(EBOOK_CONVERT_PATH_ENV) {
        return Ok((from_env, "env"));
    }
    Ok((DEFAULT_EBOOK_CONVERT_BIN.to_string(), "path"))
}

/// Resolved `ebook-convert` state for the admin Settings card: the effective
/// command, its source, and whether it is runnable. The `--version` probe
/// spawns a subprocess, so it runs on the blocking pool; a join failure is
/// reported as "not available" rather than failing the read — Calibre is
/// optional and this card must never 500.
pub async fn ebook_convert_status(pool: &SqlitePool) -> Result<EbookConvertStatus, SettingsError> {
    let (path, source) = effective_ebook_convert_path(pool).await?;
    let probe = path.clone();
    let available = match tokio::task::spawn_blocking(move || convert::is_runnable(&probe)).await {
        Ok(ok) => ok,
        Err(e) => {
            tracing::error!(error = %e, "ebook_convert_status: probe task join failed");
            false
        }
    };
    Ok(EbookConvertStatus {
        available,
        path,
        source: source.to_string(),
    })
}

/// Boot hook: seed `ebook_convert_path` from `OMNIBUS_EBOOK_CONVERT_PATH`
/// **only** when no value is already saved, so the env var works out of the box
/// without clobbering a path set through Settings on every restart (settings
/// wins).
pub async fn seed_ebook_convert_path_from_env(pool: &SqlitePool) -> Result<(), SettingsError> {
    if get_ebook_convert_path(pool).await?.is_some() {
        return Ok(());
    }
    if let Some(from_env) = non_empty_env(EBOOK_CONVERT_PATH_ENV) {
        set_ebook_convert_path(pool, Some(&from_env)).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
