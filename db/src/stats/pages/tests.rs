//! Unit tests for [`super::pages_read`] and its helpers, seeding real fixture
//! EPUBs on disk so the word-count estimate exercises actual parsing.

use std::path::Path;

use super::*;
use crate::ebook::test_support::{copy_fixture_into, fixture};
use crate::init_db;
use crate::test_support::make_test_dir;

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

/// Seed one book backed by a real fixture EPUB copied to `dir` — real bytes
/// on disk are required since `pages_read` actually opens and parses the
/// file. `dir` becomes the book's `scan_roots.path`, so `books.path` stays
/// empty and the file sits at `dir/<fixture_name>`. Idempotent on the
/// `scan_roots` row so two books can share the same `dir` (`path` is
/// UNIQUE) — callers seeding multiple fixtures into one directory don't
/// each need their own scan root.
async fn seed_book_with_fixture(pool: &SqlitePool, dir: &Path, fixture_name: &str, uuid: &str) {
    copy_fixture_into(fixture_name, dir);
    sqlx::query(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) ON CONFLICT(path) DO NOTHING",
    )
    .bind(dir.to_str().unwrap())
    .bind(dir.to_str().unwrap())
    .execute(pool)
    .await
    .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(dir.to_str().unwrap())
        .fetch_one(pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, '', ?) RETURNING id",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(uuid)
    .fetch_one(pool)
    .await
    .unwrap();
    let stem = fixture_name.trim_end_matches(".epub");
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) VALUES (?, 'EPUB', ?, 0)",
    )
    .bind(book_id)
    .bind(stem)
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

// --- words_to_pages -----------------------------------------------------

#[test]
fn words_to_pages_rounds_to_the_nearest_page() {
    assert_eq!(words_to_pages(0), 0);
    assert_eq!(words_to_pages(137), 0); // 0.498 pages
    assert_eq!(words_to_pages(138), 1); // 0.502 pages
    assert_eq!(words_to_pages(275), 1);
    assert_eq!(words_to_pages(550), 2);
}

// --- sum_word_counts ------------------------------------------------------

#[test]
fn sum_word_counts_adds_estimates_across_fixture_epubs() {
    // alpha.epub / beta.epub are real generated fixtures (see
    // `ebook::wordcount::tests` for the exact per-file word counts).
    let paths = vec![fixture("alpha.epub"), fixture("beta.epub")];
    assert_eq!(sum_word_counts(&paths), Some(14));
}

#[test]
fn sum_word_counts_skips_unreadable_paths_and_keeps_the_rest() {
    let paths = vec![
        std::path::PathBuf::from("/does/not/exist.epub"),
        fixture("alpha.epub"),
    ];
    assert_eq!(sum_word_counts(&paths), Some(4));
}

#[test]
fn sum_word_counts_is_none_when_every_path_fails() {
    let paths = vec![std::path::PathBuf::from("/does/not/exist.epub")];
    assert_eq!(sum_word_counts(&paths), None);
}

// --- pages_read -----------------------------------------------------------

#[tokio::test]
async fn pages_read_is_none_when_nothing_finished_in_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), None);
}

#[tokio::test]
async fn pages_read_sums_word_counts_across_finished_books() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let dir = make_test_dir("stats-pages-sum");

    seed_book_with_fixture(&pool, &dir, "alpha.epub", "uuid-alpha").await;
    seed_book_with_fixture(&pool, &dir, "beta.epub", "uuid-beta").await;
    finish_journal(&pool, user, "uuid-alpha", T0).await;
    finish_journal(&pool, user, "uuid-beta", T0).await;

    // 4 + 10 = 14 words total (see `sum_word_counts_adds_estimates_across_fixture_epubs`),
    // well under one `WORDS_PER_PAGE` — rounds to an honest zero, not
    // `None`: the window *does* have data, the estimate is just small.
    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), Some(0));

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn pages_read_ignores_finishes_outside_the_window() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let dir = make_test_dir("stats-pages-window");

    seed_book_with_fixture(&pool, &dir, "alpha.epub", "uuid-alpha").await;
    finish_journal(&pool, user, "uuid-alpha", T0).await;

    // A window starting after the finish must not see it.
    assert_eq!(pages_read(&pool, user, T0 + 1).await.unwrap(), None);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn pages_read_skips_a_finished_book_with_no_epub_file() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/nowhere', 'lib') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) VALUES ('uuid-nofile', ?, '', 'No File')",
    )
    .bind(lib_id)
    .execute(&pool)
    .await
    .unwrap();
    finish_journal(&pool, user, "uuid-nofile", T0).await;

    assert_eq!(pages_read(&pool, user, 0).await.unwrap(), None);
}

// ---------- StatsError variants (#1099) ----------

#[tokio::test]
async fn pages_read_propagates_books_error_when_the_batched_lookup_fails() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let dir = make_test_dir("stats-pages-books-error");

    seed_book_with_fixture(&pool, &dir, "alpha.epub", "uuid-alpha").await;
    finish_journal(&pool, user, "uuid-alpha", T0).await;

    // `finished_book_ids` only touches `journal_entries`/`books`, so it still
    // succeeds once `book_files` is gone — the failure surfaces from the
    // batched `book_file_paths` lookup itself (it joins `book_files`),
    // forcing the previously-untested `StatsError::Books` variant. Dropping
    // `scan_roots` instead would *also* work for the join, but SQLite runs
    // an implicit cascading `DELETE FROM` on `DROP TABLE` when FK
    // constraints are enabled — since `books.library_id` cascades from
    // `scan_roots`, that silently deletes the seeded book too, leaving
    // `finished_book_ids` empty and masking the very failure this test
    // wants to force. `book_files` is a *child* of `books` (nothing
    // references it), so dropping it can't cascade back up.
    sqlx::query("DROP TABLE book_files")
        .execute(&pool)
        .await
        .unwrap();

    let err = pages_read(&pool, user, 0).await.unwrap_err();
    assert!(matches!(err, StatsError::Books(_)), "got {err:?}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn spawn_blocking_panic_surfaces_as_pages_task_variant() {
    // The `spawn_blocking` call in `pages_read` runs `sum_word_counts` off
    // the async runtime; a panic there (corrupt zip state, an allocation
    // failure) must propagate as `StatsError::PagesTask` via `?` rather than
    // being swallowed. Forcing an actual EPUB-parse panic isn't practical
    // deterministically, so this exercises the identical `spawn_blocking` +
    // `?` shape `pages_read` uses, with a closure that panics on purpose.
    async fn forced() -> Result<(), StatsError> {
        tokio::task::spawn_blocking(|| panic!("intentional test panic")).await?;
        Ok(())
    }

    let err = forced().await.unwrap_err();
    assert!(matches!(err, StatsError::PagesTask(_)), "got {err:?}");
}
