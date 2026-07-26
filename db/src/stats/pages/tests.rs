//! Unit tests for [`super::pages_read`] and [`super::words_to_pages`]. The
//! word count is read straight from `books.word_count` (persisted at index
//! time), so these seed that column directly — no EPUB parsing at query time.

use super::*;
use crate::init_db;

const T0: i64 = 1_700_000_000;

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

async fn seed_lib(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Seed one `books` row with an explicit `word_count` (NULL when `None`).
async fn seed_book(pool: &SqlitePool, lib_id: i64, uuid: &str, word_count: Option<i64>) {
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, word_count)
         VALUES (?, ?, '', ?, ?)",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(uuid)
    .bind(word_count)
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

// --- words_to_pages -----------------------------------------------------

#[test]
fn words_to_pages_rounds_to_the_nearest_page() {
    assert_eq!(words_to_pages(0), 0);
    assert_eq!(words_to_pages(137), 0); // 0.498 pages
    assert_eq!(words_to_pages(138), 1); // 0.502 pages
    assert_eq!(words_to_pages(275), 1);
    assert_eq!(words_to_pages(550), 2);
}

// --- pages_read ---------------------------------------------------------

#[tokio::test]
async fn pages_read_is_none_when_nothing_finished_in_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // A finished-count-eligible book exists but is never marked finished.
    seed_book(&pool, lib, "uuid-a", Some(550)).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_sums_word_counts_of_finished_books_as_pages() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", Some(275)).await; // 1 page
    seed_book(&pool, lib, "uuid-b", Some(550)).await; // 2 pages
    finish_journal(&pool, user, "uuid-a", T0).await;
    finish_journal(&pool, user, "uuid-b", T0).await;

    // (275 + 550) / 275 = 3 pages.
    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(3));
}

#[tokio::test]
async fn pages_read_counts_a_book_finished_via_read_status() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", Some(825)).await; // 3 pages
                                                      // Finished via read-status only (no journal entry) — the tile uses the
                                                      // same unified completion definition as the rest of the stats page.
    finish_read_status(&pool, user, "uuid-a", T0).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(3));
}

#[tokio::test]
async fn pages_read_counts_a_book_finished_both_ways_once() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", Some(550)).await; // 2 pages
    finish_journal(&pool, user, "uuid-a", T0).await;
    finish_read_status(&pool, user, "uuid-a", T0).await;

    // DISTINCT-book subquery collapses the two completion events to one row,
    // so the word count is counted once (2 pages, not 4).
    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(2));
}

#[tokio::test]
async fn pages_read_is_none_when_finished_book_has_no_stored_word_count() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    // Finished, but word_count is NULL (audio-only / not-yet-backfilled).
    seed_book(&pool, lib, "uuid-a", None).await;
    finish_journal(&pool, user, "uuid-a", T0).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_ignores_finishes_outside_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", Some(550)).await;
    finish_journal(&pool, user, "uuid-a", T0).await;

    // A window starting after the finish must not see it.
    assert_eq!(pages_read(&pool, user, T0 + 1).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_ignores_a_ghosted_book_with_no_live_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    // Completion event references a uuid with no `books` row (ghosted): the
    // inner join drops it, so there is no data.
    finish_journal(&pool, user, "uuid-ghost", T0).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), None);
}
