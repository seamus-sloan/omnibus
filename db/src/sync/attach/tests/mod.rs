//! Tests for cross-format book attachment, split by sub-topic into the
//! sibling modules below; the ebook/audiobook seeding fixtures live here.
//! Covers matching a new audiobook/ebook to an existing book of the other
//! format by normalized title + author, and what later scans do to the
//! attachment once it exists.

mod matching;
mod multipart;
mod rescan;

async fn seed_ebook(pool: &sqlx::SqlitePool, filename: &str, title: &str, author: &str) {
    crate::test_support::seed_synced_ebook(pool, filename, title, author).await;
}

async fn seed_audiobook(pool: &sqlx::SqlitePool, group_path: &str, title: &str, author: &str) {
    crate::test_support::seed_synced_audiobook(pool, group_path, title, author).await;
}
