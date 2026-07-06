//! Reading/listening progress save + fetch and batched session-report ingest.

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::{ProgressFormat, ProgressRecord, ProgressUpdate, SessionReport};

#[cfg(feature = "server")]
use omnibus_db as db;

// Only the server-side body of `rpc_record_sessions` (and its test) reference
// this cap. Gate the import to keep the `web`/`mobile` client builds clean.
#[cfg(feature = "server")]
use omnibus_shared::SESSION_BATCH_CAP;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Progress-sync save. Mobile uses the analogous REST route in
/// `server::backend::progress`. POST because Dioxus `#[get]` server
/// functions can't carry an argument body — same rationale as
/// `rpc_get_ebook`; the next two endpoints follow the same pattern.
#[post("/api/rpc/progress", pool: PoolExt, user: AuthUser)]
pub async fn rpc_save_progress(update: ProgressUpdate) -> Result<ProgressRecord> {
    if let Err(msg) = update.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    match db::progress::upsert_progress(&pool.0, user.id, &update).await {
        Ok(rec) => Ok(rec),
        Err(db::progress::ProgressError::BookNotFound) => {
            Err(ServerFnError::new("book not found").into())
        }
        Err(db::progress::ProgressError::Sqlx(e)) => {
            Err(internal_rpc_error("save progress", e).into())
        }
    }
}

/// Fetch the saved reading position for `(user, book uuid, format)`. Returns
/// `Ok(None)` when the book is unknown or has no progress row for that
/// format yet — the client treats both as "start from the beginning".
#[post("/api/rpc/progress/get", pool: PoolExt, user: AuthUser)]
pub async fn rpc_get_progress(
    uuid: String,
    format: ProgressFormat,
) -> Result<Option<ProgressRecord>> {
    Ok(db::progress::get_progress(&pool.0, user.id, &uuid, format).await?)
}

/// Reject over-cap session batches at the RPC boundary, mirroring the mobile
/// REST route's 422 rejection in `server::backend::progress::post_sessions`.
/// Extracted so the length check can be covered by a unit test that doesn't
/// spin up the fullstack server-function router.
///
/// Only the server-side body of `rpc_record_sessions` calls this, so it is
/// `server`-gated like `validate_author_photo_url` — otherwise it is dead
/// code in the `mobile`/`web` client builds (caught by clippy).
#[cfg(feature = "server")]
fn check_session_batch_cap(reports: &[SessionReport]) -> Result<(), ServerFnError> {
    if reports.len() > SESSION_BATCH_CAP {
        return Err(ServerFnError::new(format!(
            "batch too large: {} records exceeds maximum of {}",
            reports.len(),
            SESSION_BATCH_CAP
        )));
    }
    Ok(())
}

/// Persist a batch of reading- or listening-session reports and return the
/// inserted count. Batches larger than `SESSION_BATCH_CAP` are rejected up
/// front (mirroring the mobile REST route in `server::backend::progress`).
/// Validates every report before any insert; the inserts then run
/// sequentially (not in a single transaction), so a DB error mid-batch
/// commits the rows that already succeeded and propagates the error to the
/// caller — the count of committed rows is then lost. Reports whose
/// `book_uuid` is unknown are silently dropped (counted out) rather than
/// failing the batch.
#[post("/api/rpc/progress/sessions", pool: PoolExt, user: AuthUser)]
pub async fn rpc_record_sessions(reports: Vec<SessionReport>) -> Result<u64> {
    check_session_batch_cap(&reports)?;
    for r in &reports {
        if let Err(msg) = r.validate() {
            return Err(ServerFnError::new(msg).into());
        }
    }
    let mut inserted = 0u64;
    for r in &reports {
        if db::progress::record_session(&pool.0, user.id, r).await? {
            inserted += 1;
        }
    }
    Ok(inserted)
}

// `server`-gated because these tests exercise `check_session_batch_cap`, which
// only exists in the server build. CI runs the frontend suite as
// `cargo test -p omnibus-frontend --features server`.
#[cfg(all(test, feature = "server"))]
mod tests {
    use super::{check_session_batch_cap, SESSION_BATCH_CAP};
    use omnibus_shared::{ProgressFormat, SessionReport};

    fn dummy_report() -> SessionReport {
        SessionReport {
            book_uuid: "uuid".into(),
            format: ProgressFormat::Epub,
            started_at: 0,
            ended_at: 1,
            progress_units: 1,
            device_id: None,
        }
    }

    #[test]
    fn check_session_batch_cap_accepts_batch_at_cap() {
        // Boundary: exactly at the cap must be accepted so a client packing
        // batches to the documented maximum isn't rejected off-by-one.
        let reports = vec![dummy_report(); SESSION_BATCH_CAP];
        assert!(check_session_batch_cap(&reports).is_ok());
    }

    #[test]
    fn check_session_batch_cap_rejects_batch_over_cap() {
        // Mirrors the mobile REST path's 422 rejection in
        // `server::backend::progress::post_sessions` — the web RPC path
        // must not permit an unbounded per-record write loop.
        let reports = vec![dummy_report(); SESSION_BATCH_CAP + 1];
        let err = check_session_batch_cap(&reports).unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains(&SESSION_BATCH_CAP.to_string()),
            "error message should name the cap: {msg}"
        );
        assert!(
            msg.contains(&(SESSION_BATCH_CAP + 1).to_string()),
            "error message should name the batch length: {msg}"
        );
    }
}
