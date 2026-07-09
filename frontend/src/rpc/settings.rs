//! Settings, worker-status, on-disk library, Hardcover-key, and audiobook
//! chapter-backfill server functions.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
use omnibus_shared::{
    HardcoverKeyStatus, LibraryContents, Settings, SmtpConfigStatus, SmtpConfigUpdate, WorkerStatus,
};

#[cfg(feature = "server")]
use omnibus_db::{self as db, scanner};

#[cfg(feature = "server")]
use super::{internal_rpc_error, AdminUser, AuthUser, PoolExt, WorkerExt};

/// Fetch the current server settings row. Admin-only.
#[get("/api/rpc/settings", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_get_settings() -> Result<Settings> {
    Ok(db::get_settings(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get settings", e))?)
}

/// Persist server settings and return the saved row. Admin-only. On success,
/// kicks off a reindex of any configured ebook / audiobook library path via
/// the shared `Worker` so concurrent saves serialize per-path; returns an
/// error when `Settings::validate()` rejects the payload.
#[post("/api/rpc/settings", pool: PoolExt, worker: WorkerExt, _admin: AdminUser)]
pub async fn rpc_save_settings(settings: Settings) -> Result<Settings> {
    if let Err(e) = settings.validate() {
        return Err(ServerFnError::new(e.to_string()).into());
    }
    db::set_settings(&pool.0, &settings)
        .await
        .map_err(|e| internal_rpc_error("save settings", e))?;
    let updated = db::get_settings(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get settings", e))?;
    // Library path may have changed (and even when it hasn't, the user has
    // signalled they want to pick up on-disk changes). Hand the reindex
    // off to the shared Worker so concurrent saves serialize per-path.
    if let Some(library_path) = updated.ebook_library_path.clone() {
        worker
            .0
            .post(omnibus_db::worker::Task::Scan { library_path });
    }
    if let Some(library_path) = updated.audiobook_library_path.clone() {
        worker
            .0
            .post(omnibus_db::worker::Task::ScanAudiobooks { library_path });
    }
    Ok(updated)
}

/// Snapshot of every in-flight and recently-completed background-worker
/// task. Polled at 1 Hz by the `WorkerStatusIndicator` component to
/// surface scan / thumbnail / author-photo / (future) cleanup progress
/// under the Save button on `/settings`.
///
/// Auth-gated as `AuthUser` (not `AdminUser`): scans affect the shared
/// library and every authed user has a reason to know one is running.
/// Worker progress snapshot for the in-page indicator.
///
/// Modeled on `rpc_save_settings` because both routes need the
/// `WorkerExt` extension. Empirically the Dioxus fullstack server-fn
/// macro mounts `#[post]` routes whose extractor list contains
/// `WorkerExt`, but the `#[get]` variant of the same signature 404s at
/// runtime (the route string ends up in the binary but never reaches
/// the axum router). The body is idempotent and read-only; using POST
/// is purely a framework workaround so the route actually mounts.
///
/// `_pool: PoolExt` is kept in the extractor list to put the macro on
/// the same code path every other `/api/rpc/*` route uses (all of
/// which take a `PoolExt`), but is unused — no DB calls run.
#[post("/api/rpc/worker_status", _pool: PoolExt, worker: WorkerExt, _user: AuthUser)]
pub async fn rpc_worker_status() -> Result<WorkerStatus> {
    Ok(worker.0.progress_snapshot())
}

/// Scan the configured ebook and audiobook library paths from disk and
/// return the raw directory contents (without DB indexing). Used by the
/// settings page to show what's on disk before reindex completes.
#[get("/api/rpc/library", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_get_library() -> Result<LibraryContents> {
    let settings = db::get_settings(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get settings", e))?;
    Ok(scanner::scan_libraries(
        settings.ebook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
    ))
}

/// Admin-only: masked status of the server-wide Hardcover key for Settings.
/// Never returns the raw key.
#[get("/api/rpc/hardcover-key", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_get_hardcover_key() -> Result<HardcoverKeyStatus> {
    Ok(db::hardcover_key_status(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get hardcover key status", e))?)
}

/// Admin-only: save (or clear, with `None`/blank) the Hardcover key in
/// settings. Returns the new masked status — never echoes the raw key.
/// Rejects tokens longer than `HARDCOVER_API_KEY_MAX_LEN` before the KV
/// write, surfacing the validation message via `ServerFnError` (same shape
/// every other RPC uses today; upgrading the codebase-wide 500-vs-4xx shape
/// is tracked in `docs/review/backend.md`).
#[post("/api/rpc/hardcover-key", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_set_hardcover_key(key: Option<String>) -> Result<HardcoverKeyStatus> {
    match db::set_hardcover_api_key(&pool.0, key.as_deref()).await {
        Ok(()) => Ok(db::hardcover_key_status(&pool.0)
            .await
            .map_err(|e| internal_rpc_error("get hardcover key status", e))?),
        Err(db::SettingsError::Validation(msg)) => Err(ServerFnError::new(msg).into()),
        Err(e) => Err(internal_rpc_error("set hardcover key", e).into()),
    }
}

/// Admin-only: masked status of the server-wide SMTP config (F4.3). Never
/// returns the raw password.
#[get("/api/rpc/smtp", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_get_smtp_config() -> Result<SmtpConfigStatus> {
    Ok(db::smtp_status(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get smtp status", e))?)
}

/// Admin-only: save the server-wide SMTP config, returning the new masked
/// status. A `None` password preserves the stored secret. Validation failures
/// surface via `ServerFnError` (same shape every other RPC uses today).
#[post("/api/rpc/smtp", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_set_smtp_config(update: SmtpConfigUpdate) -> Result<SmtpConfigStatus> {
    match db::set_smtp_config(&pool.0, &update).await {
        Ok(()) => Ok(db::smtp_status(&pool.0)
            .await
            .map_err(|e| internal_rpc_error("get smtp status", e))?),
        Err(db::SettingsError::Validation(msg)) => Err(ServerFnError::new(msg).into()),
        Err(e) => Err(internal_rpc_error("set smtp config", e).into()),
    }
}

/// Admin-only: clear the server-wide SMTP config. Returns the (now unset)
/// masked status.
#[post("/api/rpc/smtp/clear", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_clear_smtp_config() -> Result<SmtpConfigStatus> {
    db::clear_smtp_config(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("clear smtp config", e))?;
    Ok(db::smtp_status(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get smtp status", e))?)
}

/// Admin-only: send a test email to the admin's own configured Kindle address
/// to verify the SMTP config works. Requires the admin to have set their Kindle
/// email on their account page first.
#[post("/api/rpc/smtp/test", pool: PoolExt, admin: AdminUser)]
pub async fn rpc_send_smtp_test() -> Result<()> {
    let email = db::auth::get_kindle_email(&pool.0, admin.0.id)
        .await
        .map_err(|e| internal_rpc_error("get kindle email", e))?;
    let Some(email) = email else {
        return Err(ServerFnError::new(
            "set your Kindle email on your account page first, then send a test",
        )
        .into());
    };
    db::kindle::send_test(&pool.0, &email)
        .await
        .map_err(|e| internal_rpc_error("send smtp test", e))?;
    Ok(())
}

/// Admin: manually trigger a rescan of the configured ebook and audiobook
/// library paths. Posts `Task::Scan` / `Task::ScanAudiobooks` to the shared
/// worker (same tasks a settings save would) and returns immediately;
/// progress surfaces via the `WorkerStatusIndicator`. Errors when neither
/// library path is configured.
#[post("/api/rpc/scan-library", pool: PoolExt, worker: WorkerExt, _admin: AdminUser)]
pub async fn rpc_scan_library() -> Result<()> {
    let Settings {
        ebook_library_path,
        audiobook_library_path,
    } = db::get_settings(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get settings", e))?;
    if ebook_library_path.is_none() && audiobook_library_path.is_none() {
        return Err(ServerFnError::new("no library path configured").into());
    }
    if let Some(library_path) = ebook_library_path {
        worker
            .0
            .post(omnibus_db::worker::Task::Scan { library_path });
    }
    if let Some(library_path) = audiobook_library_path {
        worker
            .0
            .post(omnibus_db::worker::Task::ScanAudiobooks { library_path });
    }
    Ok(())
}

/// Admin: manually trigger chapter extraction for audiobooks missing
/// chapters. Posts `Task::BackfillChapters` to the background worker and
/// returns immediately.
#[post("/api/rpc/backfill-chapters", pool: PoolExt, worker: WorkerExt, _admin: AdminUser)]
pub async fn rpc_backfill_chapters() -> Result<()> {
    let settings = db::get_settings(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get settings", e))?;
    let Some(library_path) = settings.audiobook_library_path else {
        return Err(ServerFnError::new("no audiobook library configured").into());
    };
    worker
        .0
        .post(omnibus_db::worker::Task::BackfillChapters { library_path });
    Ok(())
}
