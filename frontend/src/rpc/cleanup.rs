//! Library-cleanup review surface: the admin-gated server functions behind
//! the Settings section and the `/settings/cleanup/:kind` review page. Thin
//! wrappers over `db::cleanup` — the review queue itself lives there.

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::{CleanupCounts, CleanupKind, Decision, IgnoredAuthor, SuggestionCard};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AdminUser, PoolExt, WorkerExt};

/// Admin-only: pending/accepted/rejected counts for each cleanup kind, in a
/// stable order, for the Settings section's per-kind rows.
#[post("/api/rpc/cleanup/counts", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_cleanup_counts() -> Result<Vec<(CleanupKind, CleanupCounts)>> {
    Ok(db::cleanup::review_counts(&pool.0)
        .await
        .map_err(|e| map_store_error("cleanup counts", e))?)
}

/// Admin-only: the pending review queue for one kind, oldest first, hydrated
/// into renderable cards. `limit` is clamped by `db::cleanup::REVIEW_QUEUE_MAX`.
#[post("/api/rpc/cleanup/queue", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_cleanup_queue(kind: CleanupKind, limit: i64) -> Result<Vec<SuggestionCard>> {
    Ok(db::cleanup::review_queue(&pool.0, kind, limit)
        .await
        .map_err(|e| map_store_error("cleanup queue", e))?)
}

/// Admin-only: record a review decision on one suggestion. `Accepted` also
/// runs the matching apply primitive, so a card the admin accepted is applied
/// before this returns.
#[post("/api/rpc/cleanup/decide", pool: PoolExt, admin: AdminUser)]
pub async fn rpc_cleanup_decide(id: i64, decision: Decision) -> Result<()> {
    db::cleanup::decide_suggestion(&pool.0, id, decision, admin.0.id)
        .await
        .map_err(|e| map_store_error("cleanup decide", e))?;
    Ok(())
}

/// Admin-only: queue a detection pass. `kind = None` runs every detector.
/// Returns the worker task id so the caller can poll the shared task status.
#[post("/api/rpc/cleanup/detect", _pool: PoolExt, worker: WorkerExt, _admin: AdminUser)]
pub async fn rpc_cleanup_detect(kind: Option<CleanupKind>) -> Result<u64> {
    Ok(worker.0.post(db::worker::Task::DetectCleanup { kind }))
}

/// Admin-only: reverse an applied cleanup by its `cleanup_log` id.
#[post("/api/rpc/cleanup/undo", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_cleanup_undo(log_id: i64) -> Result<()> {
    db::cleanup::undo(&pool.0, log_id)
        .await
        .map_err(|e| map_apply_error("cleanup undo", e))?;
    Ok(())
}

/// Admin-only: delete a taxonomy entity outright, outside the suggestion
/// queue — the on-page "this author is junk" escape hatch. Returns the
/// `cleanup_log` id so the caller can offer an undo. Only authors are
/// deletable; series/tag/book-title have no delete primitive.
#[post("/api/rpc/cleanup/delete-entity", pool: PoolExt, admin: AdminUser)]
pub async fn rpc_cleanup_delete_entity(kind: CleanupKind, entity_id: i64) -> Result<i64> {
    if kind != CleanupKind::Author {
        return Err(ServerFnError::new("only authors can be deleted").into());
    }
    Ok(
        db::cleanup::apply_delete_author(&pool.0, entity_id, None, Some(admin.0.id))
            .await
            .map_err(|e| map_apply_error("cleanup delete entity", e))?,
    )
}

/// Admin-only: merge one entity into a canonical survivor outside the
/// suggestion queue — the "this is a duplicate of…" alternative to deleting
/// an author, routed through the same `apply_merge_authors` primitive an
/// accepted merge suggestion runs (moves links, records the
/// `entity_aliases` mapping, no `ignored_authors` write). Returns the
/// `cleanup_log` id so the caller can offer an undo. Author-only, mirroring
/// the delete-entity escape hatch.
#[post("/api/rpc/cleanup/merge-entity", pool: PoolExt, admin: AdminUser)]
pub async fn rpc_cleanup_merge_entity(
    kind: CleanupKind,
    source_id: i64,
    canonical_id: i64,
) -> Result<i64> {
    if kind != CleanupKind::Author {
        return Err(ServerFnError::new("only authors can be merged here").into());
    }
    Ok(db::cleanup::apply_merge_authors(
        &pool.0,
        &[source_id],
        canonical_id,
        None,
        Some(admin.0.id),
    )
    .await
    .map_err(|e| map_apply_error("cleanup merge entity", e))?)
}

/// Admin-only: every `ignored_authors` blocklist entry, for the Settings
/// blocklist management list.
#[post("/api/rpc/cleanup/ignored-authors", pool: PoolExt, _admin: AdminUser)]
pub async fn rpc_cleanup_ignored_authors() -> Result<Vec<IgnoredAuthor>> {
    Ok(db::cleanup::list_ignored_authors(&pool.0)
        .await
        .map_err(|e| map_apply_error("cleanup ignored authors", e))?)
}

/// Admin-only: convert an `ignored_authors` entry into an author alias onto
/// `canonical_id`, then queue the authorless relink pass on both libraries
/// so books orphaned while the name was blocklisted regain their author
/// link. Returns the `cleanup_log` id for undo.
#[post("/api/rpc/cleanup/alias-ignored", pool: PoolExt, worker: WorkerExt, admin: AdminUser)]
pub async fn rpc_cleanup_alias_ignored_author(name: String, canonical_id: i64) -> Result<i64> {
    let log_id =
        db::cleanup::apply_alias_ignored_author(&pool.0, &name, canonical_id, Some(admin.0.id))
            .await
            .map_err(|e| map_apply_error("cleanup alias ignored author", e))?;
    post_relink_tasks(&pool.0, &worker.0).await?;
    Ok(log_id)
}

/// Admin-only: remove an `ignored_authors` entry outright, then queue the
/// authorless relink pass so orphaned books can re-create the author from
/// their file metadata again.
#[post("/api/rpc/cleanup/remove-ignored", pool: PoolExt, worker: WorkerExt, _admin: AdminUser)]
pub async fn rpc_cleanup_remove_ignored_author(name: String) -> Result<()> {
    db::cleanup::remove_ignored_author(&pool.0, &name)
        .await
        .map_err(|e| map_apply_error("cleanup remove ignored author", e))?;
    post_relink_tasks(&pool.0, &worker.0).await?;
    Ok(())
}

/// Post `Task::RelinkAuthorless` for each configured library root. Shared
/// tail of the alias/remove routes — the blocklist change only affects
/// future parses, so the repair pass must be queued for the healing to
/// reach already-orphaned books.
#[cfg(feature = "server")]
async fn post_relink_tasks(
    pool: &sqlx::SqlitePool,
    worker: &std::sync::Arc<db::worker::Worker>,
) -> Result<(), ServerFnError> {
    let settings = db::get_settings(pool)
        .await
        .map_err(|e| internal_rpc_error("cleanup relink settings", e))?;
    if let Some(library_path) = settings.ebook_library_path {
        worker.post(db::worker::Task::RelinkAuthorless {
            library_path,
            audiobooks: false,
        });
    }
    if let Some(library_path) = settings.audiobook_library_path {
        worker.post(db::worker::Task::RelinkAuthorless {
            library_path,
            audiobooks: true,
        });
    }
    Ok(())
}

/// Map a `CleanupStoreError` to a client-facing error. The internal faults
/// (`Db`, `Payload`, `UnknownToken`) are logged and genericized — an unknown
/// token must not echo the token back; a refusal already carries a safe,
/// specific sentence.
#[cfg(feature = "server")]
fn map_store_error(context: &'static str, e: db::cleanup::CleanupStoreError) -> ServerFnError {
    use db::cleanup::CleanupStoreError as E;
    match e {
        E::Db(_) | E::Payload(_) | E::UnknownToken(_) => internal_rpc_error(context, e),
        E::Apply(inner) => map_apply_error(context, inner),
        E::Refused(msg) => ServerFnError::new(msg),
    }
}

/// Map a `CleanupApplyError` the same way: the typed variants already carry a
/// safe sentence; only the opaque ones are genericized and logged.
#[cfg(feature = "server")]
fn map_apply_error(context: &'static str, e: db::CleanupApplyError) -> ServerFnError {
    match e {
        db::CleanupApplyError::Db(inner) => internal_rpc_error(context, inner),
        db::CleanupApplyError::Snapshot(inner) => internal_rpc_error(context, inner),
        other => ServerFnError::new(other.to_string()),
    }
}

#[cfg(all(test, feature = "server"))]
mod tests;
