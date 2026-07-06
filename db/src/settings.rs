//! Settings KV CRUD, scan-root row upserts, and orphan-scan-root pruning.
//!
//! `settings` KV keys (`ebook_library_path` / `audiobook_library_path`) are
//! read by the UI and translated into `scan_roots` rows by the indexer; saving
//! settings prunes any orphan root and its books, FTS rows, and on-disk covers.

use std::path::Path;

use sqlx::{SqlitePool, Transaction};

pub use omnibus_shared::{
    is_plausible_email, HardcoverKeyStatus, Settings, SmtpConfigStatus, SmtpConfigUpdate,
    SmtpSecurity, EMAIL_MAX_LEN, HARDCOVER_API_KEY_MAX_LEN, SMTP_FIELD_MAX_LEN,
    SMTP_PASSWORD_MAX_LEN,
};

/// `settings` KV keys consumed by the UI/RPC layer. Kept as constants so the
/// indexer, settings handlers, and tests all reference the same identifier.
const EBOOK_LIBRARY_PATH_KEY: &str = "ebook_library_path";
const AUDIOBOOK_LIBRARY_PATH_KEY: &str = "audiobook_library_path";
/// `settings` KV key for the F3.3 Hardcover API token. Stored directly (not via
/// the [`Settings`] struct) so saving it never triggers the scan-root
/// reconciliation `set_settings` runs.
const HARDCOVER_API_KEY_KEY: &str = "hardcover_api_key";
/// `settings` KV keys for the F4.3 server-wide SMTP config. Stored as discrete
/// rows (not via the [`Settings`] struct) so saving them never triggers the
/// scan-root reconciliation `set_settings` runs. The password is masked before
/// it ever crosses the wire (see [`smtp_status`]).
const SMTP_HOST_KEY: &str = "smtp_host";
const SMTP_PORT_KEY: &str = "smtp_port";
const SMTP_USERNAME_KEY: &str = "smtp_username";
const SMTP_PASSWORD_KEY: &str = "smtp_password";
const SMTP_FROM_EMAIL_KEY: &str = "smtp_from_email";
const SMTP_SECURITY_KEY: &str = "smtp_security";

/// Errors returned by the settings data layer.
#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// Caller-supplied value violates a byte-length / shape constraint enforced
    /// before the write. Surfaced as 4xx by handlers so an admin sees a
    /// per-case message rather than a generic 500.
    #[error("{0}")]
    Validation(String),
}

/// Read the ebook and audiobook library paths from the `settings` KV table.
/// Missing keys map to `None` rather than an error — the first-run UI relies
/// on this to detect an unconfigured server.
pub async fn get_settings(pool: &SqlitePool) -> Result<Settings, SettingsError> {
    let ebook_library_path =
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
            .bind(EBOOK_LIBRARY_PATH_KEY)
            .fetch_optional(pool)
            .await?;
    let audiobook_library_path =
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
            .bind(AUDIOBOOK_LIBRARY_PATH_KEY)
            .fetch_optional(pool)
            .await?;
    Ok(Settings {
        ebook_library_path,
        audiobook_library_path,
    })
}

/// Persist library paths and reconcile dependent state in a single
/// transaction: upserts each path into `settings`, prunes orphaned
/// `scan_roots` rows (and their books / FTS rows via cascade), then deletes
/// the matching cover files from disk *after* commit so filesystem
/// side-effects don't run inside the DB transaction. Callers should kick
/// off a reindex afterwards — this function does not touch the indexer.
pub async fn set_settings(pool: &SqlitePool, settings: &Settings) -> Result<(), SettingsError> {
    // Read the *current* paths before writing so a changed path can be
    // repointed in place (F2) — keeping the `scan_roots` row's id, and thus
    // every `books.uuid` keyed under it, stable across the move.
    let current = get_settings(pool).await?;
    let mut tx = pool.begin().await?;
    // The other slot's *new* path is passed so a shared scan root (both slots
    // pointing at one path) isn't silently moved when only one slot changes.
    repoint_scan_root(
        &mut tx,
        current.ebook_library_path.as_deref(),
        settings.ebook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
    )
    .await?;
    repoint_scan_root(
        &mut tx,
        current.audiobook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
        settings.ebook_library_path.as_deref(),
    )
    .await?;
    upsert_or_clear(
        &mut tx,
        EBOOK_LIBRARY_PATH_KEY,
        settings.ebook_library_path.as_deref(),
    )
    .await?;
    upsert_or_clear(
        &mut tx,
        AUDIOBOOK_LIBRARY_PATH_KEY,
        settings.audiobook_library_path.as_deref(),
    )
    .await?;
    let orphan_uuids = prune_orphan_libraries(
        &mut tx,
        &[
            settings.ebook_library_path.as_deref(),
            settings.audiobook_library_path.as_deref(),
        ],
    )
    .await?;
    tx.commit().await?;

    // Unlinking the orphaned cover files is synchronous `std::fs` that scales
    // with the orphan count, so move it off the runtime. A `JoinError` (panic
    // in the unlink loop) is logged and swallowed — covers are a rebuildable
    // cache, so a failed cleanup must not fail the settings save.
    if let Err(join_err) =
        tokio::task::spawn_blocking(move || crate::covers::delete_cover_files_for(&orphan_uuids))
            .await
    {
        tracing::error!("set_settings: cover cleanup spawn_blocking failed: {join_err}");
    }
    Ok(())
}

/// Repoint a slot's `scan_roots` row in place when its configured path
/// changes (F2). Updating the existing row's `path` (rather than letting
/// `upsert_library` insert a new one) keeps the row id — and therefore every
/// `books.uuid` keyed under that `library_id` — stable, so the next reindex
/// matches each file by its unchanged relative `scan_key` and preserves its
/// identity. The attached-file location overrides (`book_files.library_path`)
/// and the attach ledger (`merged_uuids.library_path`) are repointed in the
/// same step so attachments stay discoverable and resolve to the new root.
///
/// A no-op unless both old and new paths are set and differ; skipped if a row
/// for the new path already exists (avoid a UNIQUE collision — the existing
/// row wins and the reindex reconciles), and skipped when `old` is still
/// referenced by the *other* slot's new path (a shared scan root must not be
/// moved out from under the slot that still uses it).
async fn repoint_scan_root(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    old: Option<&str>,
    new: Option<&str>,
    other_new: Option<&str>,
) -> Result<(), SettingsError> {
    let (Some(old), Some(new)) = (old, new) else {
        return Ok(());
    };
    if old == new {
        return Ok(());
    }
    // Shared-root guard: the other slot still points at `old`, so renaming the
    // row would silently move both libraries and leave the other slot dangling.
    if other_new == Some(old) {
        return Ok(());
    }
    let new_exists: Option<i64> = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(new)
        .fetch_optional(&mut **tx)
        .await?;
    if new_exists.is_some() {
        return Ok(());
    }
    let display_name = Path::new(new)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(new)
        .to_string();
    sqlx::query("UPDATE scan_roots SET path = ?, display_name = ? WHERE path = ?")
        .bind(new)
        .bind(&display_name)
        .bind(old)
        .execute(&mut **tx)
        .await?;
    // Repoint the file-location overrides + attach ledger that key on the
    // *file's* scanned root (set only for cross-format attachments) so the
    // reindex diff (`list_merged_rows_for_formats`) and `book_file_path*`
    // resolve to the new root instead of the stale one.
    sqlx::query("UPDATE merged_uuids SET library_path = ? WHERE library_path = ?")
        .bind(new)
        .bind(old)
        .execute(&mut **tx)
        .await?;
    sqlx::query("UPDATE book_files SET library_path = ? WHERE library_path = ?")
        .bind(new)
        .bind(old)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Drop **childless** orphan `scan_roots` rows whose `path` is not in `keep`.
///
/// Never-prune (F2): a removed scan root that still owns books is **kept**,
/// and its books (plus their soft-ref user data) are retained rather than
/// cascade-deleted — the durable-identity safety net. Books under a cleared
/// path simply stop being listed (no `list_books` call passes that path) and
/// reappear if it is re-added. Only roots with zero books are swept, to bound
/// empty-row accumulation; nothing user-facing is deleted, so the returned
/// uuid list (for post-commit cover cleanup) is always empty.
pub(crate) async fn prune_orphan_libraries(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    keep: &[Option<&str>],
) -> Result<Vec<String>, SettingsError> {
    let orphan_ids: Vec<i64> = sqlx::query_as("SELECT id, path FROM scan_roots")
        .fetch_all(&mut **tx)
        .await?
        .into_iter()
        .filter(|(_, path): &(i64, String)| {
            !keep.iter().any(|k| k.map(|s| s == path).unwrap_or(false))
        })
        .map(|(id, _)| id)
        .collect();

    if !orphan_ids.is_empty() {
        // One batched conditional delete: an orphan row goes only if it has no
        // books. A root that still owns books is left in place (never-prune)
        // so the FK cascade on `books.library_id` is never triggered. The
        // `NOT EXISTS` filter keeps that guard per-row inside the set delete,
        // so a book-owning orphan in the id list survives untouched. Scan
        // roots are bounded to a handful, so the id list never approaches
        // SQLite's bind cap.
        let placeholders = std::iter::repeat_n("?", orphan_ids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "DELETE FROM scan_roots
              WHERE id IN ({placeholders})
                AND NOT EXISTS (SELECT 1 FROM books WHERE library_id = scan_roots.id)"
        );
        let mut q = sqlx::query(&sql);
        for id in &orphan_ids {
            q = q.bind(id);
        }
        q.execute(&mut **tx).await?;
    }

    Ok(Vec::new())
}

async fn upsert_or_clear(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    key: &str,
    value: Option<&str>,
) -> Result<(), sqlx::Error> {
    match value {
        Some(v) => {
            sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)")
                .bind(key)
                .bind(v)
                .execute(&mut **tx)
                .await?;
        }
        None => {
            sqlx::query("DELETE FROM settings WHERE key = ?")
                .bind(key)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}

/// Boot-time hook that seeds [`Settings`] from `EBOOK_LIBRARY_PATH` /
/// `AUDIOBOOK_LIBRARY_PATH` if either is present. No-op when both are
/// unset, so production deployments that configure libraries through the
/// UI are unaffected. Delegates writes through [`set_settings`], so
/// orphan-cleanup runs the same way as a user-initiated save.
pub async fn seed_settings_from_env(pool: &SqlitePool) -> Result<(), SettingsError> {
    let ebook_library_path = std::env::var("EBOOK_LIBRARY_PATH").ok();
    let audiobook_library_path = std::env::var("AUDIOBOOK_LIBRARY_PATH").ok();
    if ebook_library_path.is_some() || audiobook_library_path.is_some() {
        set_settings(
            pool,
            &Settings {
                ebook_library_path,
                audiobook_library_path,
            },
        )
        .await?;
    }
    Ok(())
}

/// Read the Hardcover API key saved in `settings`, or `None` when unset/blank.
/// This is the raw secret — callers serving it to a client MUST mask it.
pub async fn get_hardcover_api_key(pool: &SqlitePool) -> Result<Option<String>, SettingsError> {
    let v = sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
        .bind(HARDCOVER_API_KEY_KEY)
        .fetch_optional(pool)
        .await?;
    Ok(v.filter(|s| !s.trim().is_empty()))
}

/// Persist (or clear, when `None`/blank) the Hardcover API key in `settings`.
/// Rejects tokens longer than [`HARDCOVER_API_KEY_MAX_LEN`] with
/// [`SettingsError::Validation`] before the write so no admin write path can
/// spill an unbounded blob into the `settings` KV table.
pub async fn set_hardcover_api_key(
    pool: &SqlitePool,
    key: Option<&str>,
) -> Result<(), SettingsError> {
    match key.map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) if v.len() > HARDCOVER_API_KEY_MAX_LEN => {
            return Err(SettingsError::Validation(format!(
                "hardcover api key exceeds {HARDCOVER_API_KEY_MAX_LEN} bytes"
            )));
        }
        Some(v) => {
            sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)")
                .bind(HARDCOVER_API_KEY_KEY)
                .bind(v)
                .execute(pool)
                .await?;
        }
        None => {
            sqlx::query("DELETE FROM settings WHERE key = ?")
                .bind(HARDCOVER_API_KEY_KEY)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

/// The effective Hardcover key: the saved settings value wins; the
/// `HARDCOVER_API_KEY` env var is the fallback when no value is saved. Returns
/// `None` (feature disabled) when neither is set.
pub async fn effective_hardcover_api_key(
    pool: &SqlitePool,
) -> Result<Option<String>, SettingsError> {
    if let Some(saved) = get_hardcover_api_key(pool).await? {
        return Ok(Some(saved));
    }
    Ok(std::env::var("HARDCOVER_API_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty()))
}

/// Masked status of the server-wide Hardcover key: the saved settings value
/// wins (`source = "settings"`), then the `HARDCOVER_API_KEY` env var
/// (`source = "env"`), else unset (`source = "none"`). Never returns the raw
/// key — only a short masked preview. Shared by the REST handler and the RPC
/// server function so the fallback + masking live once.
pub async fn hardcover_key_status(pool: &SqlitePool) -> Result<HardcoverKeyStatus, SettingsError> {
    if let Some(k) = get_hardcover_api_key(pool).await? {
        return Ok(HardcoverKeyStatus {
            configured: true,
            masked: Some(mask_key(&k)),
            source: "settings".to_string(),
        });
    }
    if let Ok(env_key) = std::env::var("HARDCOVER_API_KEY") {
        let env_key = env_key.trim();
        if !env_key.is_empty() {
            return Ok(HardcoverKeyStatus {
                configured: true,
                masked: Some(mask_key(env_key)),
                source: "env".to_string(),
            });
        }
    }
    Ok(HardcoverKeyStatus {
        configured: false,
        masked: None,
        source: "none".to_string(),
    })
}

/// Short masked preview of a secret — never the raw value. Keys of 8 chars or
/// fewer collapse to a fixed bullet run; longer keys (Hardcover tokens are
/// JWTs) render as `first4…last4`.
fn mask_key(key: &str) -> String {
    let n = key.chars().count();
    if n <= 8 {
        return "\u{2022}\u{2022}\u{2022}\u{2022}".to_string();
    }
    let first: String = key.chars().take(4).collect();
    let last: String = key.chars().skip(n - 4).collect();
    format!("{first}\u{2026}{last}")
}

/// Boot hook: seed the Hardcover key from `HARDCOVER_API_KEY` **only** when no
/// value is already saved, so the env var works out of the box without
/// clobbering a key set through Settings on every restart (settings wins).
pub async fn seed_hardcover_key_from_env(pool: &SqlitePool) -> Result<(), SettingsError> {
    if get_hardcover_api_key(pool).await?.is_some() {
        return Ok(());
    }
    if let Ok(env_key) = std::env::var("HARDCOVER_API_KEY") {
        if !env_key.trim().is_empty() {
            set_hardcover_api_key(pool, Some(&env_key)).await?;
        }
    }
    Ok(())
}

/// Fully-resolved SMTP config used by the F4.3 send path (`crate::kindle`).
/// Server-only — carries the raw password, so it never crosses the wire; the
/// UI sees the masked [`SmtpConfigStatus`] instead.
#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
    pub from_email: String,
    pub security: SmtpSecurity,
}

/// Read one raw `settings` value, treating blank as absent.
async fn get_setting(pool: &SqlitePool, key: &str) -> Result<Option<String>, SettingsError> {
    Ok(
        sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key = ?")
            .bind(key)
            .fetch_optional(pool)
            .await?
            .filter(|s| !s.trim().is_empty()),
    )
}

/// Read the saved SMTP config from `settings`, or `None` when the required
/// fields (host + from-email) aren't both present. Includes the raw password;
/// callers serving status to a client MUST use [`smtp_status`] instead.
pub async fn get_smtp_config(pool: &SqlitePool) -> Result<Option<SmtpConfig>, SettingsError> {
    let (Some(host), Some(from_email)) = (
        get_setting(pool, SMTP_HOST_KEY).await?,
        get_setting(pool, SMTP_FROM_EMAIL_KEY).await?,
    ) else {
        return Ok(None);
    };
    Ok(Some(SmtpConfig {
        host,
        port: smtp_port_or_default(get_setting(pool, SMTP_PORT_KEY).await?.as_deref()),
        username: get_setting(pool, SMTP_USERNAME_KEY)
            .await?
            .unwrap_or_default(),
        password: get_setting(pool, SMTP_PASSWORD_KEY)
            .await?
            .unwrap_or_default(),
        from_email,
        security: SmtpSecurity::from_str_or_default(
            &get_setting(pool, SMTP_SECURITY_KEY)
                .await?
                .unwrap_or_default(),
        ),
    }))
}

/// Parse a stored/env SMTP port, defaulting to 587 (STARTTLS submission) for a
/// missing or unparseable value.
fn smtp_port_or_default(raw: Option<&str>) -> u16 {
    raw.and_then(|s| s.trim().parse().ok()).unwrap_or(587)
}

/// Persist the SMTP config from an admin request, validating each field before
/// the write. A `password` of `None` preserves the stored secret (so an admin
/// can edit the host without re-typing it); `Some("")` clears it. Host, port,
/// username, from-email, and security are always overwritten with the request
/// values.
pub async fn set_smtp_config(
    pool: &SqlitePool,
    update: &SmtpConfigUpdate,
) -> Result<(), SettingsError> {
    let host = update.host.trim();
    let from_email = update.from_email.trim();
    let username = update.username.trim();
    if host.is_empty() || host.len() > SMTP_FIELD_MAX_LEN {
        return Err(SettingsError::Validation(format!(
            "smtp host must be 1..={SMTP_FIELD_MAX_LEN} bytes"
        )));
    }
    // `port` is a `u16`, so the upper bound is free; reject 0 explicitly so a
    // never-connectable config can't be persisted (matches the UI's 1..=65535).
    if update.port == 0 {
        return Err(SettingsError::Validation(
            "smtp port must be 1..=65535".to_string(),
        ));
    }
    if username.len() > SMTP_FIELD_MAX_LEN {
        return Err(SettingsError::Validation(format!(
            "smtp username exceeds {SMTP_FIELD_MAX_LEN} bytes"
        )));
    }
    if !is_plausible_email(from_email) {
        return Err(SettingsError::Validation(
            "smtp from-email is not a valid email address".to_string(),
        ));
    }
    if let Some(pw) = &update.password {
        if pw.len() > SMTP_PASSWORD_MAX_LEN {
            return Err(SettingsError::Validation(format!(
                "smtp password exceeds {SMTP_PASSWORD_MAX_LEN} bytes"
            )));
        }
    }

    let mut tx = pool.begin().await?;
    upsert_or_clear(&mut tx, SMTP_HOST_KEY, Some(host)).await?;
    upsert_or_clear(&mut tx, SMTP_PORT_KEY, Some(&update.port.to_string())).await?;
    upsert_or_clear(&mut tx, SMTP_USERNAME_KEY, Some(username)).await?;
    upsert_or_clear(&mut tx, SMTP_FROM_EMAIL_KEY, Some(from_email)).await?;
    upsert_or_clear(&mut tx, SMTP_SECURITY_KEY, Some(update.security.as_str())).await?;
    // `None` preserves the existing password row; `Some(v)` (incl. empty) sets
    // or clears it.
    if let Some(pw) = &update.password {
        upsert_or_clear(&mut tx, SMTP_PASSWORD_KEY, Some(pw.as_str())).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Delete every SMTP `settings` row (admin "Clear" action).
pub async fn clear_smtp_config(pool: &SqlitePool) -> Result<(), SettingsError> {
    let mut tx = pool.begin().await?;
    for key in [
        SMTP_HOST_KEY,
        SMTP_PORT_KEY,
        SMTP_USERNAME_KEY,
        SMTP_PASSWORD_KEY,
        SMTP_FROM_EMAIL_KEY,
        SMTP_SECURITY_KEY,
    ] {
        upsert_or_clear(&mut tx, key, None).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// The effective SMTP config: the saved settings value wins; the `SMTP_*` env
/// vars are the fallback when nothing is saved. Returns `None` (feature
/// disabled) when neither source has the required host + from-email.
pub async fn effective_smtp_config(pool: &SqlitePool) -> Result<Option<SmtpConfig>, SettingsError> {
    if let Some(saved) = get_smtp_config(pool).await? {
        return Ok(Some(saved));
    }
    Ok(smtp_config_from_env())
}

/// Assemble an [`SmtpConfig`] from `SMTP_*` env vars, or `None` when host /
/// from-email aren't both set. Mirrors the settings-row precedence used by
/// [`get_smtp_config`].
fn smtp_config_from_env() -> Option<SmtpConfig> {
    let host = non_empty_env("SMTP_HOST")?;
    let from_email = non_empty_env("SMTP_FROM_EMAIL")?;
    Some(SmtpConfig {
        host,
        port: smtp_port_or_default(non_empty_env("SMTP_PORT").as_deref()),
        username: non_empty_env("SMTP_USERNAME").unwrap_or_default(),
        password: non_empty_env("SMTP_PASSWORD").unwrap_or_default(),
        from_email,
        security: SmtpSecurity::from_str_or_default(
            &non_empty_env("SMTP_SECURITY").unwrap_or_default(),
        ),
    })
}

/// Read a trimmed, non-empty env var, or `None`.
fn non_empty_env(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// Masked status of the server-wide SMTP config: saved settings win
/// (`source = "settings"`), then `SMTP_*` env vars (`source = "env"`), else
/// unset (`source = "none"`). The password is only ever returned masked.
pub async fn smtp_status(pool: &SqlitePool) -> Result<SmtpConfigStatus, SettingsError> {
    let (config, source) = match get_smtp_config(pool).await? {
        Some(c) => (Some(c), "settings"),
        None => match smtp_config_from_env() {
            Some(c) => (Some(c), "env"),
            None => (None, "none"),
        },
    };
    let Some(c) = config else {
        return Ok(SmtpConfigStatus {
            source: "none".to_string(),
            ..Default::default()
        });
    };
    Ok(SmtpConfigStatus {
        configured: true,
        host: Some(c.host),
        port: Some(c.port),
        username: Some(c.username).filter(|s| !s.is_empty()),
        from_email: Some(c.from_email),
        security: c.security,
        password_masked: Some(c.password)
            .filter(|s| !s.is_empty())
            .map(|p| mask_key(&p)),
        source: source.to_string(),
    })
}

/// Boot hook: seed the SMTP config from `SMTP_*` env vars **only** when no host
/// is already saved, so the env vars work out of the box without clobbering a
/// config set through Settings on every restart (settings wins).
pub async fn seed_smtp_from_env(pool: &SqlitePool) -> Result<(), SettingsError> {
    if get_setting(pool, SMTP_HOST_KEY).await?.is_some() {
        return Ok(());
    }
    if let Some(config) = smtp_config_from_env() {
        set_smtp_config(
            pool,
            &SmtpConfigUpdate {
                host: config.host,
                port: config.port,
                username: config.username,
                from_email: config.from_email,
                security: config.security,
                password: Some(config.password),
            },
        )
        .await?;
    }
    Ok(())
}

/// Upsert a `scan_roots` row for `path` (display_name derived from basename).
/// Used by the indexer write path in `sync`.
pub(crate) async fn upsert_library(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    path: &str,
) -> Result<i64, SettingsError> {
    let display_name = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string();
    sqlx::query(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?)
         ON CONFLICT(path) DO UPDATE SET display_name = excluded.display_name",
    )
    .bind(path)
    .bind(&display_name)
    .execute(&mut **tx)
    .await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(path)
        .fetch_one(&mut **tx)
        .await?;
    Ok(id)
}

/// Unix-seconds timestamp of the last successful index for `library_path`,
/// or `None` if the library has never been indexed (or doesn't exist in the
/// `scan_roots` table yet).
pub async fn last_indexed_at(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Option<i64>, SettingsError> {
    Ok(
        sqlx::query_scalar::<_, Option<i64>>("SELECT last_indexed FROM scan_roots WHERE path = ?")
            .bind(library_path)
            .fetch_optional(pool)
            .await?
            .flatten(),
    )
}

#[cfg(test)]
mod tests;
