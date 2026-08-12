//! Reading/listening progress save + fetch and batched session-report ingest.

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::{
    AudiobookPlaybackRateRecord, AudiobookPlaybackRateUpdate, ProgressFormat, ProgressRecord,
    ProgressUpdate, ResumePoint, SessionReport,
};

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
    Ok(db::progress::get_progress(&pool.0, user.id, &uuid, format)
        .await
        .map_err(|e| internal_rpc_error("get progress", e))?)
}

/// Save the current user's playback rate for one audiobook.
#[post("/api/rpc/audiobooks/playback-rate/set", pool: PoolExt, user: AuthUser)]
pub async fn rpc_set_playback_rate(
    uuid: String,
    update: AudiobookPlaybackRateUpdate,
) -> Result<AudiobookPlaybackRateRecord> {
    if let Err(message) = update.validate() {
        return Err(ServerFnError::new(message).into());
    }
    match db::progress::set_playback_rate(&pool.0, user.id, &uuid, &update).await {
        Ok(record) => Ok(record),
        Err(db::progress::ProgressError::BookNotFound) => {
            Err(ServerFnError::new("book not found").into())
        }
        Err(db::progress::ProgressError::Sqlx(e)) => {
            Err(internal_rpc_error("set playback rate", e).into())
        }
    }
}

/// Fetch the current user's playback rate for one audiobook.
#[post("/api/rpc/audiobooks/playback-rate/get", pool: PoolExt, user: AuthUser)]
pub async fn rpc_get_playback_rate(uuid: String) -> Result<Option<AudiobookPlaybackRateRecord>> {
    match db::progress::get_playback_rate(&pool.0, user.id, &uuid).await {
        Ok(record) => Ok(record),
        Err(db::progress::ProgressError::BookNotFound) => {
            Err(ServerFnError::new("book not found").into())
        }
        Err(db::progress::ProgressError::Sqlx(e)) => {
            Err(internal_rpc_error("get playback rate", e).into())
        }
    }
}

/// The user's most recent progress rows joined with their books — the
/// "pick up where you left off" feed. Mirrors the mobile REST route
/// `GET /api/progress/recent`; `limit` is clamped to the same 1..=20 range.
#[post("/api/rpc/progress/recent", pool: PoolExt, user: AuthUser)]
pub async fn rpc_recent_progress(limit: i64) -> Result<Vec<ResumePoint>> {
    const LIMIT_CAP: i64 = 20;
    let limit = limit.clamp(1, LIMIT_CAP);
    Ok(db::progress::resume_points(&pool.0, user.id, limit)
        .await
        .map_err(|e| internal_rpc_error("recent progress", e))?)
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

/// Single-transaction batch insert shared by `rpc_record_sessions` and its
/// tests. Pre-resolves every `book_uuid` in one bulk query, then loops pure
/// inserts and commits once — mirroring the mobile REST route
/// (`server::backend::progress::post_sessions`), so a DB error mid-batch
/// rolls the whole batch back and the client can safely retry. Reports whose
/// `book_uuid` is unknown are silently skipped; the returned count reflects
/// only the rows that landed.
#[cfg(feature = "server")]
async fn record_sessions_batch(
    pool: &sqlx::SqlitePool,
    user_id: i64,
    reports: &[SessionReport],
) -> Result<u64, ServerFnError> {
    // `BEGIN IMMEDIATE`: this is the web/mobile path behind the
    // `POST /api/rpc/progress/sessions` 500 in #1862 — the bulk-resolve
    // below reads before the insert loop writes, so a DEFERRED transaction
    // risks `SQLITE_BUSY_SNAPSHOT` (517) when two devices post batches
    // concurrently. See the matching comment on
    // `db::progress::upsert_progress`.
    let mut tx = pool
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(|e| internal_rpc_error("begin sessions batch", e))?;
    let batch_uuids: Vec<String> = reports.iter().map(|r| r.book_uuid.clone()).collect();
    let resolved = db::resolve_canonical_book_uuids_bulk_exec(&mut tx, &batch_uuids)
        .await
        .map_err(|e| internal_rpc_error("resolve sessions batch", e))?;
    let mut inserted = 0u64;
    for r in reports {
        let Some(canonical) = resolved.get(&r.book_uuid) else {
            continue;
        };
        db::progress::insert_session_tx(&mut tx, user_id, r, canonical)
            .await
            .map_err(|e| internal_rpc_error("insert session", e))?;
        inserted += 1;
    }
    tx.commit()
        .await
        .map_err(|e| internal_rpc_error("commit sessions batch", e))?;
    Ok(inserted)
}

/// Persist a batch of reading- or listening-session reports and return the
/// inserted count. Batches larger than `SESSION_BATCH_CAP` are rejected up
/// front and every report is validated before any insert (mirroring the
/// mobile REST route in `server::backend::progress`); the inserts then run
/// inside a single transaction via `record_sessions_batch`.
#[post("/api/rpc/progress/sessions", pool: PoolExt, user: AuthUser)]
pub async fn rpc_record_sessions(reports: Vec<SessionReport>) -> Result<u64> {
    check_session_batch_cap(&reports)?;
    for r in &reports {
        if let Err(msg) = r.validate() {
            return Err(ServerFnError::new(msg).into());
        }
    }
    Ok(record_sessions_batch(&pool.0, user.id, &reports).await?)
}

// `server`-gated because these tests exercise `check_session_batch_cap`, which
// only exists in the server build. CI runs the frontend suite as
// `cargo test -p omnibus-frontend --features server`.
#[cfg(all(test, feature = "server"))]
mod tests {
    use super::{check_session_batch_cap, record_sessions_batch, SESSION_BATCH_CAP};
    use omnibus_shared::{ProgressFormat, SessionReport};

    fn dummy_report() -> SessionReport {
        report("uuid", ProgressFormat::Epub)
    }

    fn report(book_uuid: &str, format: ProgressFormat) -> SessionReport {
        SessionReport {
            book_uuid: book_uuid.into(),
            format,
            started_at: 0,
            ended_at: 1,
            progress_units: 1,
            device_id: None,
            client_id: None,
        }
    }

    #[tokio::test]
    async fn record_sessions_batch_skips_unknown_uuid_and_counts_only_inserted_rows() {
        let pool = omnibus_db::init_db("sqlite::memory:").await.unwrap();
        omnibus_db::test_support::seed_minimal_books(&pool, 2).await;
        let user_id: i64 = sqlx::query_scalar(
            "INSERT INTO users (username, password_hash) VALUES ('alice', 'x') RETURNING id",
        )
        .fetch_one(&pool)
        .await
        .unwrap();

        let reports = vec![
            report("uuid-1", ProgressFormat::Epub),
            report("no-such-book", ProgressFormat::Epub),
            report("uuid-2", ProgressFormat::Audio),
        ];
        let inserted = record_sessions_batch(&pool, user_id, &reports)
            .await
            .unwrap();
        assert_eq!(inserted, 2, "unknown uuid must be skipped, not counted");

        let reading: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reading_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        let listening: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM listening_sessions")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!((reading, listening), (1, 1));
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
