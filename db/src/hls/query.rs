//! DB query layer for the HLS handlers: resolve a book uuid (with optional
//! `book_files.id`) to the ids and library path needed downstream, plus
//! the per-file `book_file_parts` + `file_chapters` lookups the manifest
//! builder and status handler depend on.

use std::collections::HashMap;

use sqlx::SqlitePool;

use super::{HlsError, HlsPart, ResolvedAudiobook};

/// SQLite's default bound-parameter cap is 999; chunking well under it
/// keeps a single `IN (...)` query safe regardless of build-time overrides.
const BULK_CHUNK_SIZE: usize = 500;

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

/// Number of audio-format `book_files` rows `book_id` carries. The format
/// list mirrors [`resolve_audiobook_file`] — this counts the candidates that
/// resolver chooses between, so `> 1` marks a multi-file audiobook whose
/// positions are only meaningful together with a `book_file_id`.
pub async fn count_audio_files(pool: &SqlitePool, book_id: i64) -> Result<i64, HlsError> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(*) FROM book_files \
         WHERE book_id = ? AND format IN ('M4B', 'M4A', 'MP3')",
    )
    .bind(book_id)
    .fetch_one(pool)
    .await?)
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

/// Fetch ordered `book_file_parts` for every id in `book_file_ids` in one
/// round trip (chunked under SQLite's bind cap), grouped by `book_file_id`.
/// A file with no rows in the map has no parts.
pub async fn get_parts_bulk(
    pool: &SqlitePool,
    book_file_ids: &[i64],
) -> Result<HashMap<i64, Vec<HlsPart>>, HlsError> {
    let mut map: HashMap<i64, Vec<HlsPart>> = HashMap::new();
    for chunk in book_file_ids.chunks(BULK_CHUNK_SIZE) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT book_file_id, ordinal, filename, duration_seconds \
             FROM book_file_parts \
             WHERE book_file_id IN ({placeholders}) \
             ORDER BY book_file_id, ordinal"
        );
        let mut q = sqlx::query_as::<_, (i64, i64, String, f64)>(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        for (book_file_id, ordinal, filename, duration_seconds) in q.fetch_all(pool).await? {
            map.entry(book_file_id).or_default().push(HlsPart {
                ordinal,
                filename,
                duration_seconds,
            });
        }
    }
    Ok(map)
}

/// Fetch chapter markers for every id in `book_file_ids` in one round trip
/// (chunked under SQLite's bind cap), grouped by `book_file_id`. Returns
/// only rows actually found in `file_chapters` — an id with no chapters (or
/// not present in `book_files` at all) is simply absent from the returned
/// map. Unlike [`get_chapters`]'s single-id query, which always returns a
/// `Vec` (possibly empty) for the one id it's given, a caller here must
/// treat a missing key as "no chapters", not panic or assume the id was
/// invalid.
pub async fn get_chapters_bulk(
    pool: &SqlitePool,
    book_file_ids: &[i64],
) -> Result<HashMap<i64, Vec<omnibus_shared::ChapterInfo>>, HlsError> {
    let mut map: HashMap<i64, Vec<omnibus_shared::ChapterInfo>> = HashMap::new();
    for chunk in book_file_ids.chunks(BULK_CHUNK_SIZE) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let sql = format!(
            "SELECT book_file_id, ordinal, title, start_seconds, duration_seconds \
             FROM file_chapters \
             WHERE book_file_id IN ({placeholders}) \
             ORDER BY book_file_id, ordinal"
        );
        let mut q = sqlx::query_as::<_, (i64, i64, String, f64, f64)>(&sql);
        for id in chunk {
            q = q.bind(id);
        }
        for (book_file_id, ordinal, title, start_seconds, duration_seconds) in
            q.fetch_all(pool).await?
        {
            map.entry(book_file_id)
                .or_default()
                .push(omnibus_shared::ChapterInfo {
                    ordinal,
                    title,
                    start_seconds,
                    duration_seconds,
                });
        }
    }
    Ok(map)
}
