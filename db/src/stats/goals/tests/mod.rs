//! Unit tests for `db::stats::goals`, split by scope into the sibling
//! modules below; the user and completion seeding fixtures they share live
//! here. Covers both goal scopes' read/write paths, every `GoalError`
//! variant, the calendar each daily kind is measured over, and the cache
//! invalidation a just-saved goal depends on.

mod annual;
mod daily_calendar;
mod daily_targets;

use super::*;

async fn seed_user(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, '!x', 0, 0, 0, 1) RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Seed a user with an explicit id. The stats cache is a process-wide static
/// keyed on `(user_id, range)` and every test pool restarts ids at 1, so a
/// test exercising the *cached* `user_stats` entry point claims an id no
/// sibling test can collide with — a `clear_cache()` here would race the
/// sibling TTL test rather than help.
async fn seed_user_with_id(pool: &SqlitePool, id: i64, name: &str) -> i64 {
    sqlx::query("INSERT INTO users (id, username, password_hash) VALUES (?, ?, '!x')")
        .bind(id)
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    id
}

/// Give a seeded book an exact page count — rung 2 of the length ladder, and
/// the cheapest to seed without inventing a word count.
async fn set_pages(pool: &SqlitePool, uuid: &str, pages: i64) {
    sqlx::query("UPDATE books SET page_count = ? WHERE uuid = ?")
        .bind(pages)
        .bind(uuid)
        .execute(pool)
        .await
        .unwrap();
}

/// Accrue a day's forward progress in the `0083` ledger, as the progress write
/// path does. `day` is UTC `YYYY-MM-DD`, which is the only calendar this table
/// has.
async fn accrue(pool: &SqlitePool, user: i64, uuid: &str, day: &str, percent: i64) {
    sqlx::query(
        "INSERT INTO reading_progress_daily (user_id, book_uuid, format, day, percent_gained)
         VALUES (?, ?, 'epub', ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(day)
    .bind(percent)
    .execute(pool)
    .await
    .unwrap();
}

/// Today's UTC date, `YYYY-MM-DD`.
async fn utc_day(pool: &SqlitePool) -> String {
    sqlx::query_scalar("SELECT date('now')")
        .fetch_one(pool)
        .await
        .unwrap()
}
