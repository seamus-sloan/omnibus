//! Unit tests for the `progress` module, split by sub-topic into the sibling
//! modules below; the shared book/user/audio-file seeding fixtures live here.

mod concurrency;
mod derive_percent;
mod kobo_statistics;
mod playback_rate;
mod resume;
mod session;
mod state;
mod upsert;
mod upsert_audio;
mod upsert_logging;

use omnibus_shared::{EbookMetadata, ProgressUpdate};
use sqlx::SqlitePool;

use crate::replace_books;

use super::*;

/// Map a merged/auto-attached `uuid` onto an existing `book_id` the way the
/// merge transaction does (`db/src/merge/transaction.rs`), so the session path
/// has a row to resolve through the `merged_uuids` UNION fallback.
async fn seed_merged_uuid(pool: &SqlitePool, uuid: &str, book_id: i64, format: &str) {
    sqlx::query(
        "INSERT OR REPLACE INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES (?, ?, ?, ?)",
    )
    .bind(uuid)
    .bind(book_id)
    .bind(format)
    .bind("/lib")
    .execute(pool)
    .await
    .expect("seed merged uuid");
}

async fn seed(pool: &SqlitePool, library: &str, title: &str) -> (i64, String) {
    replace_books(
        pool,
        library,
        vec![crate::ebook::IndexedBook {
            metadata: EbookMetadata {
                filename: format!("{title}.epub").to_lowercase(),
                title: Some(title.to_string()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
            word_count: None,
        }],
    )
    .await
    .expect("seed book");
    let books = crate::list_books(pool, library).await.unwrap();
    let book = books
        .into_iter()
        .find(|b| b.title.as_deref() == Some(title))
        .unwrap();
    (book.id, book.unique_identifier.clone().unwrap())
}

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

/// Insert `n` audio `book_files` rows for `book_id`, returning their ids.
/// Gives the multi-file guard in `upsert_progress_tx` real audio rows to
/// count (the `seed` helper's EPUB file doesn't participate).
async fn seed_audio_files(pool: &SqlitePool, book_id: i64, n: usize) -> Vec<i64> {
    let mut ids = Vec::with_capacity(n);
    for ordinal in 0..n {
        let id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, ordinal)
             VALUES (?, 'M4B', ?, 1, ?) RETURNING id",
        )
        .bind(book_id)
        .bind(format!("part-{ordinal}.m4b"))
        .bind(ordinal as i64)
        .fetch_one(pool)
        .await
        .unwrap();
        ids.push(id);
    }
    ids
}

/// Shorthand for an audio `ProgressUpdate` in the multi-file guard tests.
fn audio_update(
    uuid: &str,
    seconds: f64,
    book_file_id: Option<i64>,
    client_updated_at: i64,
) -> ProgressUpdate {
    ProgressUpdate {
        book_uuid: uuid.to_string(),
        format: ProgressFormat::Audio,
        epub_cfi: None,
        audio_position_seconds: Some(seconds),
        progress_percent: None,
        kobo_location: None,
        book_file_id,
        client_updated_at: Some(client_updated_at),
    }
}

/// Insert a row directly with a NULL `client_updated_at` — the column has
/// no `NOT NULL` constraint, and rows written by a path other than
/// `upsert_progress` (or a pre-migration row not yet backfilled) can
/// legitimately have one.
async fn seed_null_client_updated_at(pool: &SqlitePool, user_id: i64, uuid: &str, updated_at: i64) {
    sqlx::query(
        "INSERT INTO reading_progress
            (user_id, book_uuid, format, epub_cfi, updated_at, client_updated_at)
         VALUES (?, ?, 'epub', 'epubcfi(null-client-ts)', ?, NULL)",
    )
    .bind(user_id)
    .bind(uuid)
    .bind(updated_at)
    .execute(pool)
    .await
    .unwrap();
}

/// Seed an audiobook (books + book_files + parts + chapters) with two 600 s
/// parts and three chapters, returning its uuid.
async fn seed_audiobook(pool: &SqlitePool, uuid: &str) -> i64 {
    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/ab', 'ab')")
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, '/ab', 'A')")
            .bind(uuid)
            .bind(lib_id)
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid();
    let file_id = sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'M4B', 'a', 1)",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid();
    for (ordinal, dur) in [(0i64, 600.0f64), (1, 600.0)] {
        sqlx::query(
            "INSERT INTO book_file_parts \
                (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
             VALUES (?, ?, 'p', 1, 0, ?)",
        )
        .bind(file_id)
        .bind(ordinal)
        .bind(dur)
        .execute(pool)
        .await
        .unwrap();
    }
    for (ordinal, start, dur) in [
        (1i64, 0.0f64, 400.0f64),
        (2, 400.0, 400.0),
        (3, 800.0, 400.0),
    ] {
        sqlx::query(
            "INSERT INTO file_chapters \
                (book_file_id, ordinal, title, start_seconds, duration_seconds) \
             VALUES (?, ?, 'ch', ?, ?)",
        )
        .bind(file_id)
        .bind(ordinal)
        .bind(start)
        .bind(dur)
        .execute(pool)
        .await
        .unwrap();
    }
    book_id
}

/// Seed an epub position for `(user, uuid)` so the row shows up on the rail.
async fn seed_epub_position(pool: &SqlitePool, user: i64, uuid: &str) {
    upsert_progress(
        pool,
        user,
        &ProgressUpdate {
            book_uuid: uuid.to_string(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: None,
        },
    )
    .await
    .expect("seed position");
}

/// Attach a second audiobook file to `book_id` — a different narration of
/// the same book: one 3000 s part and two chapters, so its totals can't be
/// confused with the first file's (1200 s / three chapters).
async fn seed_second_audiobook_file(pool: &SqlitePool, book_id: i64) -> i64 {
    let file_id = sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, ordinal) \
         VALUES (?, 'M4B', 'b', 1, 1)",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_file_parts \
            (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
         VALUES (?, 0, 'p', 1, 0, 3000.0)",
    )
    .bind(file_id)
    .execute(pool)
    .await
    .unwrap();
    for (ordinal, start, dur) in [(1i64, 0.0f64, 1500.0f64), (2, 1500.0, 1500.0)] {
        sqlx::query(
            "INSERT INTO file_chapters \
                (book_file_id, ordinal, title, start_seconds, duration_seconds) \
             VALUES (?, ?, 'ch', ?, ?)",
        )
        .bind(file_id)
        .bind(ordinal)
        .bind(start)
        .bind(dur)
        .execute(pool)
        .await
        .unwrap();
    }
    file_id
}
