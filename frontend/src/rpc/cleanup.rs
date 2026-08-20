//! Library-cleanup review surface: the admin-gated server functions behind
//! the Settings section and the `/settings/cleanup/:kind` review page. Thin
//! wrappers over `db::cleanup` — the review queue itself lives there.

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::{CleanupCounts, CleanupKind, Decision, SuggestionCard};

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
