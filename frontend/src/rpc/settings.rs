//! Settings, worker-status, on-disk library, Hardcover-key, and audiobook
//! chapter-backfill server functions.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
use omnibus_shared::{
    HardcoverKeyStatus, LibraryContents, MetadataSource, Settings, SmtpConfigStatus,
    SmtpConfigUpdate, WorkerStatus,
};

#[cfg(feature = "server")]
use omnibus_db::{self as db, scanner};

// Only referenced from the server-only bodies the `#[get]`/`#[post]` macros
// splice in under `#[cfg(feature = "server")]` — the client stub compiled
// for `web`/`mobile` never calls them, so gate the imports to match or a
// non-`server` build sees them as unused.
#[cfg(feature = "server")]
use omnibus_shared::{is_valid_metadata_precedence, DEFAULT_METADATA_PRECEDENCE};

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

/// Resolve the `library` discriminator (`"ebook"` or `"audiobook"`) used by
/// the metadata-precedence RPCs below to the corresponding configured path.
/// A separate get/set pair per concern (mirroring the Hardcover-key / SMTP
/// RPCs) keeps this F5.1 (#972) setting out of the `Settings` struct, whose
/// exhaustive field literals are used across dozens of call sites.
#[cfg(feature = "server")]
fn precedence_library_path(
    settings: &Settings,
    library: &str,
) -> Result<Option<String>, ServerFnError> {
    match library {
        "ebook" => Ok(settings.ebook_library_path.clone()),
        "audiobook" => Ok(settings.audiobook_library_path.clone()),
        _ => Err(ServerFnError::new(
            "library must be \"ebook\" or \"audiobook\"",
        )),
    }
}

/// Fetch the metadata-source precedence for `library` (`"ebook"` or
/// `"audiobook"`). Returns the default order when the library path isn't
/// configured yet, so the settings UI always has something to render.
#[post("/api/rpc/metadata-precedence/get", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_get_metadata_precedence(library: String) -> Result<Vec<MetadataSource>> {
    let settings = db::get_settings(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get settings", e))?;
    let path = precedence_library_path(&settings, &library)?;
    match path {
        Some(p) => Ok(db::get_metadata_precedence(&pool.0, &p)
            .await
            .map_err(|e| internal_rpc_error("get metadata precedence", e))?),
        None => Ok(DEFAULT_METADATA_PRECEDENCE.to_vec()),
    }
}

/// Persist the metadata-source precedence for `library` (`"ebook"` or
/// `"audiobook"`). Requires the library's path to already be configured
/// (the setting lives on its `scan_roots` row) and rejects a precedence
/// list that isn't a permutation of the 5 known sources.
#[post("/api/rpc/metadata-precedence/set", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_set_metadata_precedence(
    library: String,
    precedence: Vec<MetadataSource>,
) -> Result<Vec<MetadataSource>> {
    if !is_valid_metadata_precedence(&precedence) {
        return Err(
            ServerFnError::new("metadata precedence must list each source exactly once").into(),
        );
    }
    let settings = db::get_settings(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get settings", e))?;
    let Some(path) = precedence_library_path(&settings, &library)? else {
        return Err(ServerFnError::new(
            "configure the library path before setting its metadata precedence",
        )
        .into());
    };
    db::set_metadata_precedence(&pool.0, &path, &precedence)
        .await
        .map_err(|e| internal_rpc_error("set metadata precedence", e))?;
    Ok(precedence)
}

/// Snapshot of every in-flight and recently-completed background-worker
/// task. Polled at 1 Hz by the `WorkerStatusIndicator` component to
/// surface scan / thumbnail / author-photo / (future) cleanup progress
/// under the Save button on `/settings`.
///
/// Auth-gated as `AuthUser` (not `AdminUser`): scans affect the shared
/// library and every authed user has a reason to know one is running.
/// Scoped to the caller via [`omnibus_db::worker::Worker::owner_scoped_snapshot`]
/// so a user-owned task (e.g. another user's Send-to-Kindle failure) never
/// reaches a caller who doesn't own it — only unowned, library-wide tasks
/// are visible to everyone.
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
#[post("/api/rpc/worker_status", _pool: PoolExt, worker: WorkerExt, user: AuthUser)]
pub async fn rpc_worker_status() -> Result<WorkerStatus> {
    Ok(worker.0.owner_scoped_snapshot(user.id))
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

/// Admin-only: masked status of the server-wide SMTP config. Never
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
        scan_interval_hours: _,
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
