//! Unit tests for `db::stats`, split by sub-topic into the sibling modules
//! below; the session, link, rating and completion seeding fixtures they
//! share live here (and are reused by `builder/tests`).

mod finished;
mod genres_ratings;
mod summary;
mod windows;

use omnibus_shared::StatsRange;

use super::*;

pub(super) const DAY: i64 = 86_400;

pub(super) async fn seed_user(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, '!x', 0, 0, 0, 1) RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// The `books.id` behind a seeded `uuid-N`, for the taxonomy link helpers.
pub(super) async fn book_id(pool: &SqlitePool, uuid: &str) -> i64 {
    sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap()
}

pub(super) async fn reading_session(
    pool: &SqlitePool,
    user: i64,
    uuid: &str,
    started_at: i64,
    secs: i64,
) {
    sqlx::query(
        "INSERT INTO reading_sessions (user_id, book_uuid, started_at, ended_at, seconds_read)
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

pub(super) async fn listening_session(
    pool: &SqlitePool,
    user: i64,
    uuid: &str,
    started_at: i64,
    secs: i64,
) {
    sqlx::query(
        "INSERT INTO listening_sessions (user_id, book_uuid, started_at, ended_at, seconds_listened)
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

async fn link_author(pool: &SqlitePool, book: i64, name: &str) {
    sqlx::query("INSERT OR IGNORE INTO authors (name) VALUES (?)")
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    let author: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO books_authors_link (book, author, position) VALUES (?, ?, 0)")
        .bind(book)
        .bind(author)
        .execute(pool)
        .await
        .unwrap();
}

async fn link_tag(pool: &SqlitePool, book: i64, name: &str) {
    sqlx::query("INSERT OR IGNORE INTO tags (name) VALUES (?)")
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    let tag: i64 = sqlx::query_scalar("SELECT id FROM tags WHERE name = ?")
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO books_tags_link (book, tag) VALUES (?, ?)")
        .bind(book)
        .bind(tag)
        .execute(pool)
        .await
        .unwrap();
}

/// Assign genres to a book. Genres have no link table (migration `0066`) —
/// they exist only as a `metadata_overrides` entry — so this goes through
/// the real write path, which also materializes the `genres` rows the
/// donut's join needs.
pub(super) async fn set_genres(pool: &SqlitePool, uuid: &str, names: &[&str], user: i64) {
    crate::merge_metadata_overrides(
        pool,
        uuid,
        &omnibus_shared::MetadataOverrides {
            genres: Some(names.iter().map(|n| (*n).to_string()).collect()),
            ..Default::default()
        },
        user,
    )
    .await
    .unwrap();
}

pub(super) async fn rate_book(
    pool: &SqlitePool,
    user: i64,
    uuid: &str,
    half_stars: i64,
    updated_at: i64,
) {
    sqlx::query(
        "INSERT INTO user_ratings (user_id, book_uuid, half_stars, updated_at)
         VALUES (?, ?, ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(half_stars)
    .bind(updated_at)
    .execute(pool)
    .await
    .unwrap();
}

/// Unix seconds `months` calendar-months before the DB's `now` — computed by
/// SQLite itself so it lands in the same calendar month the trailing-12
/// recursive CTE anchors on, regardless of day-of-month clamping.
pub(super) async fn months_ago_secs(pool: &SqlitePool, months: i64) -> i64 {
    sqlx::query_scalar(&format!(
        // Mid-month anchor: naive '-N months' from a month-end 'now'
        // (July 31 → "June 31" → July 1) lands the seed in the wrong month.
        "SELECT CAST(strftime('%s', 'now', 'start of month', '-{months} months', '+14 days') AS INTEGER)"
    ))
    .fetch_one(pool)
    .await
    .unwrap()
}

pub(super) async fn finish_journal(pool: &SqlitePool, user: i64, uuid: &str, created_at: i64) {
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

/// A late-2023 anchor: an all-time window covers it, "this year" (2026+) does not.
const T0: i64 = 1_700_000_000;

/// Stamp an explicit read-status `finished` on a book at `finished_at`.
async fn finish_read_status(pool: &SqlitePool, user: i64, uuid: &str, finished_at: i64) {
    sqlx::query(
        "INSERT INTO book_read_status (user_id, book_uuid, status, updated_at, finished_at)
         VALUES (?, ?, 'finished', ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(finished_at)
    .bind(finished_at)
    .execute(pool)
    .await
    .unwrap();
}

/// Drop a book row the way `merge::transaction::finalize_merge` used to,
/// leaving its soft-referencing user-data rows behind. Migration `0079` heals
/// the rows an old merge stranded and the merge path no longer strands new
/// ones — but a completion can outlive its book by any route that drops a
/// `books` row, and every metric must agree about that book either way.
async fn drop_book_row(pool: &SqlitePool, uuid: &str) {
    sqlx::query("DELETE FROM books WHERE uuid = ?")
        .bind(uuid)
        .execute(pool)
        .await
        .unwrap();
}

/// A moment safely inside the current calendar month. Not `now - 60`: for the
/// first minute of the 1st that lands in the *previous* month, while the
/// trailing-12 series still ends on the current one.
async fn this_month_secs(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar("SELECT CAST(strftime('%s','now','start of month','+1 day') AS INTEGER)")
        .fetch_one(pool)
        .await
        .unwrap()
}

/// First second of the period preceding `range`'s current one.
async fn prev_period_start(pool: &SqlitePool, range: StatsRange) -> i64 {
    prev_window_bounds(pool, range, 0).await.unwrap().unwrap().0
}
