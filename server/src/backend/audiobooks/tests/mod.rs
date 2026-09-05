//! Integration tests for the audiobook streaming endpoints, split by route
//! into the sibling modules below; the seeded-audiobook fixture they share
//! lives here. Covers the direct-play manifest, Range-served parts, the
//! download routes, the legacy HLS fallback, the playback-rate store and
//! the content validators, each with auth gating and 4xx / 5xx paths.

mod download;
mod hls;
mod manifest;
mod parts;
mod playback_rate;
mod validators;

/// Seed one audiobook book + book_files + book_file_parts row for tests.
async fn seed_one_audiobook(pool: &sqlx::SqlitePool) -> String {
    let lib_id =
        sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'audiobooks')")
            .bind("/audiobooks")
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid();
    let uuid = "aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee";
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'PK')")
            .bind(uuid)
            .bind(lib_id)
            .bind("/audiobooks")
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid();
    let file_id = sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'MP3', 'the-princess-knight', 100)",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_file_parts (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
         VALUES (?, 0, 'ch01.mp3', 50, 0, 300.0)",
    )
    .bind(file_id)
    .execute(pool)
    .await
    .unwrap();
    uuid.to_string()
}
