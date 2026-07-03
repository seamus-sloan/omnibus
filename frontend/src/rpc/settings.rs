//! Settings, worker-status, on-disk library, Hardcover-key, and audiobook
//! chapter-backfill server functions.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
use omnibus_shared::{HardcoverKeyStatus, LibraryContents, Settings, WorkerStatus};

#[cfg(feature = "server")]
use omnibus_db::{self as db, scanner};

#[cfg(feature = "server")]
use super::{AdminUser, AuthUser, PoolExt, WorkerExt};

/// Fetch the current server settings row. Admin-only.
#[get("/api/rpc/settings", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_get_settings() -> Result<Settings> {
    Ok(db::get_settings(&pool.0).await?)
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
    db::set_settings(&pool.0, &settings).await?;
    let updated = db::get_settings(&pool.0).await?;
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
    let settings = db::get_settings(&pool.0).await?;
    Ok(scanner::scan_libraries(
        settings.ebook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
    ))
}

/// Admin-only: masked status of the server-wide Hardcover key for Settings.
/// Never returns the raw key.
#[get("/api/rpc/hardcover-key", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_get_hardcover_key() -> Result<HardcoverKeyStatus> {
    Ok(db::hardcover_key_status(&pool.0).await?)
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
        Ok(()) => Ok(db::hardcover_key_status(&pool.0).await?),
        Err(db::SettingsError::Validation(msg)) => Err(ServerFnError::new(msg).into()),
        Err(e) => Err(ServerFnError::new(e.to_string()).into()),
    }
}

/// Admin: manually trigger chapter extraction for audiobooks missing
/// chapters. Posts `Task::BackfillChapters` to the background worker and
/// returns immediately.
#[post("/api/rpc/backfill-chapters", pool: PoolExt, worker: WorkerExt, _admin: AdminUser)]
pub async fn rpc_backfill_chapters() -> Result<()> {
    let settings = db::get_settings(&pool.0).await?;
    let Some(library_path) = settings.audiobook_library_path else {
        return Err(ServerFnError::new("no audiobook library configured").into());
    };
    worker
        .0
        .post(omnibus_db::worker::Task::BackfillChapters { library_path });
    Ok(())
}
