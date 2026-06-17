//! DB query layer for the HLS handlers: resolve a book uuid (with optional
//! `book_files.id`) to the ids and library path needed downstream, plus
//! the per-file `book_file_parts` + `file_chapters` lookups the manifest
//! builder and status handler depend on.

use sqlx::SqlitePool;

use super::{HlsError, HlsPart, ResolvedAudiobook};

/// Resolve a book `uuid` to the ids and library path needed by the HLS
/// handlers. When `file_id` is `Some`, resolve that specific `book_files`
/// row (verifying it belongs to the given uuid and is an audio format).
/// When `None`, returns the first audio file by ordinal. Returns `None`
/// when the uuid is unknown or no matching audio file exists.
pub async fn resolve_audiobook(
    pool: &SqlitePool,
    uuid: &str,
) -> Result<Option<ResolvedAudiobook>, HlsError> {
    resolve_audiobook_file(pool, uuid, None).await
}

/// Inner resolver that optionally targets a specific `book_files.id`.
pub async fn resolve_audiobook_file(
    pool: &SqlitePool,
    uuid: &str,
    file_id: Option<i64>,
) -> Result<Option<ResolvedAudiobook>, HlsError> {
    let row = if let Some(fid) = file_id {
        sqlx::query_as::<_, (i64, i64, String)>(
            "SELECT b.id, bf.id, COALESCE(bf.library_path, l.path) \
             FROM books b \
             JOIN book_files bf ON bf.book_id = b.id \
             JOIN scan_roots l ON l.id = b.library_id \
             WHERE b.uuid = ? \
               AND bf.id = ? \
               AND bf.format IN ('M4B', 'M4A', 'MP3')",
        )
        .bind(uuid)
        .bind(fid)
        .fetch_optional(pool)
        .await?
    } else {
        sqlx::query_as::<_, (i64, i64, String)>(
            "SELECT b.id, bf.id, COALESCE(bf.library_path, l.path) \
             FROM books b \
             JOIN book_files bf ON bf.book_id = b.id \
             JOIN scan_roots l ON l.id = b.library_id \
             WHERE b.uuid = ? \
               AND bf.format IN ('M4B', 'M4A', 'MP3') \
             ORDER BY bf.ordinal \
             LIMIT 1",
        )
        .bind(uuid)
        .fetch_optional(pool)
        .await?
    };

    Ok(
        row.map(|(book_id, book_file_id, library_path)| ResolvedAudiobook {
            book_id,
            book_file_id,
            library_path,
        }),
    )
}

/// Fetch ordered `book_file_parts` for `book_file_id`.
pub async fn get_parts(pool: &SqlitePool, book_file_id: i64) -> Result<Vec<HlsPart>, HlsError> {
    let rows = sqlx::query_as::<_, (i64, String, f64)>(
        "SELECT ordinal, filename, duration_seconds \
         FROM book_file_parts \
         WHERE book_file_id = ? \
         ORDER BY ordinal",
    )
    .bind(book_file_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|(ordinal, filename, duration_seconds)| HlsPart {
            ordinal,
            filename,
            duration_seconds,
        })
        .collect())
}

/// Fetch chapter markers for a `book_file_id`. Always returns at least one
/// row because the sync layer writes synthetic chapters when none are
/// extracted from the container.
pub async fn get_chapters(
    pool: &SqlitePool,
    book_file_id: i64,
) -> Result<Vec<omnibus_shared::ChapterInfo>, HlsError> {
    let rows = sqlx::query_as::<_, (i64, String, f64, f64)>(
        "SELECT ordinal, title, start_seconds, duration_seconds \
         FROM file_chapters \
         WHERE book_file_id = ? \
         ORDER BY ordinal",
    )
    .bind(book_file_id)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(
            |(ordinal, title, start_seconds, duration_seconds)| omnibus_shared::ChapterInfo {
                ordinal,
                title,
                start_seconds,
                duration_seconds,
            },
        )
        .collect())
}
