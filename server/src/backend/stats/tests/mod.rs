//! Tests for the reading-stats REST handlers, split by route into the
//! sibling modules below; the session seeding fixtures they share live
//! here. The `db::stats` cache is process-wide and keyed on
//! `(user_id, range, offset_minutes)`, and every fixture pool restarts user
//! ids at 1 — so each content-asserting test uses a distinct range (or a
//! distinct resolved offset) to keep its cache key unique across the binary.

mod goals;
mod session_log;
mod summary;

async fn seed_reading_session(
    pool: &sqlx::SqlitePool,
    user: i64,
    uuid: &str,
    started_at: i64,
    secs: i64,
) {
    seed_reading_session_at_offset(pool, user, uuid, started_at, secs, None).await;
}

/// A sitting carrying the capture-time offset column (migration `0080`) — what
/// the day-boundary fallback reads when a request declares no offset of its own.
async fn seed_reading_session_at_offset(
    pool: &sqlx::SqlitePool,
    user: i64,
    uuid: &str,
    started_at: i64,
    secs: i64,
    utc_offset_minutes: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO reading_sessions
             (user_id, book_uuid, started_at, ended_at, seconds_read, utc_offset_minutes)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(started_at)
    .bind(started_at + secs)
    .bind(secs)
    .bind(utc_offset_minutes)
    .execute(pool)
    .await
    .unwrap();
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}
