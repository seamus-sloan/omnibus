//! Unit tests for the length ladder and the aggregates over it, split by
//! sub-topic into the sibling modules below; the book, session, completion
//! and ledger seeding fixtures they share live here. Every input is a
//! persisted column, so these seed those columns directly — no EPUB or CBZ
//! is opened, and the ledger is seeded rather than driven through the
//! position write path (covered in `db::progress::ledger`).

mod detail;
mod ledger_days;
mod length_buckets;
mod pages_per_hour;
mod pages_read;

use super::*;

const T0: i64 = 1_700_000_000;

async fn seed_lib(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Seed one `books` row with explicit `word_count` / `page_count` (NULL when
/// `None`) — the two lower rungs of the ladder.
async fn seed_book(
    pool: &SqlitePool,
    lib_id: i64,
    uuid: &str,
    word_count: Option<i64>,
    page_count: Option<i64>,
) {
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, word_count, page_count)
         VALUES (?, ?, '', ?, ?, ?)",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(uuid)
    .bind(word_count)
    .bind(page_count)
    .execute(pool)
    .await
    .unwrap();
}

/// The ladder's top rung: a print-edition page count in the override blob.
async fn set_print_pages(pool: &SqlitePool, uuid: &str, print_pages: i64) {
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides)
         VALUES (?, json_object('print_pages', ?))",
    )
    .bind(uuid)
    .bind(print_pages)
    .execute(pool)
    .await
    .unwrap();
}

async fn finish_journal(pool: &SqlitePool, user: i64, uuid: &str, created_at: i64) {
    sqlx::query(
        "INSERT INTO journal_entries (user_id, book_uuid, body_md, progress, created_at)
         VALUES (?, ?, 'done', 100, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(created_at)
    .execute(pool)
    .await
    .unwrap();
}

async fn finish_read_status(pool: &SqlitePool, user: i64, uuid: &str, finished_at: i64) {
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, finished_at)
         VALUES (?, ?, 'finished', ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(finished_at)
    .execute(pool)
    .await
    .unwrap();
}

async fn listen_session(pool: &SqlitePool, user: i64, uuid: &str, started_at: i64, secs: i64) {
    sqlx::query(
        "INSERT INTO listening_sessions
             (user_id, book_uuid, started_at, ended_at, seconds_listened)
         VALUES (?, ?, ?, ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(started_at)
    .bind(started_at + secs)
    .bind(secs)
    .execute(pool)
    .await
    .unwrap();
}

/// Seed one day's forward progress on a book in the **frozen** ledger
/// (migration `0083`).
///
/// Still exercised because those rows are still read: they keep only a day
/// string, so the union passes them through on whatever calendar they were
/// written with rather than re-bucketing them. New writes go to
/// [`read_percent_at`] instead.
async fn read_percent(pool: &SqlitePool, user: i64, uuid: &str, day: &str, gained: i64) {
    sqlx::query(
        "INSERT INTO reading_progress_daily
             (user_id, book_uuid, format, day, percent_gained)
         VALUES (?, ?, 'epub', ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(day)
    .bind(gained)
    .execute(pool)
    .await
    .unwrap();
}

/// Seed forward progress observed at a unix instant — the current ledger
/// (migration `0095`), which keys on the quarter-hour so the day stays a read-
/// time question.
async fn read_percent_at(pool: &SqlitePool, user: i64, uuid: &str, at: i64, gained: i64) {
    sqlx::query(
        "INSERT INTO reading_progress_slots
             (user_id, book_uuid, format, slot, percent_gained)
         VALUES (?, ?, 'epub', ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(at.div_euclid(crate::progress::SLOT_SECS))
    .bind(gained)
    .execute(pool)
    .await
    .unwrap();
}

/// The UTC day `T0` falls in — every ledger row here is seeded on it unless a
/// test is specifically about day bucketing.
const T0_DAY: &str = "2023-11-14";
