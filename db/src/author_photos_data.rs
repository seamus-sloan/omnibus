//! Author profile photo data layer plus the admin `delete_author`
//! primitive. `author_photos` holds at most one row per author (PK =
//! author_id) with a `source` of `'manual'`, `'openlibrary'`, or
//! `'letter'` (negative-cache marker — NULL bytes/mime so `get_author_photo`
//! returns `None` and the letter avatar stays in place). Admin DELETE
//! clears the row to force re-resolution. The OL resolver itself lives in
//! [`crate::author_photos`]; this file owns just the row CRUD.

use sqlx::SqlitePool;

use crate::metadata_overrides::rebuild_fts_for_books_batch;

/// Errors returned by the author-photos data layer. The single transparent
/// `Db` variant wraps `sqlx::Error` at the module boundary per the
/// `02-error-handling` boundary rule, keeping `?` propagation clean at
/// every call site.
#[derive(Debug, thiserror::Error)]
pub enum AuthorPhotosDataError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Source-of-truth marker for a cached author photo row.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthorPhotoSource {
    Manual,
    OpenLibrary,
    Letter,
}

impl AuthorPhotoSource {
    /// Return the database string representation of this photo source variant.
    pub fn as_str(&self) -> &'static str {
        match self {
            AuthorPhotoSource::Manual => "manual",
            AuthorPhotoSource::OpenLibrary => "openlibrary",
            AuthorPhotoSource::Letter => "letter",
        }
    }

    /// Parse a database string into an `AuthorPhotoSource` variant; returns `None` for unrecognised values.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "manual" => Some(Self::Manual),
            "openlibrary" => Some(Self::OpenLibrary),
            "letter" => Some(Self::Letter),
            _ => None,
        }
    }
}

/// Fetch a cached profile photo for the given author. Returns `None` when no
/// row exists or the row is a `'letter'` negative-cache marker — both cases
/// should produce a 404 from the GET handler so the frontend keeps rendering
/// the letter avatar.
pub async fn get_author_photo(
    pool: &SqlitePool,
    author_id: i64,
) -> Result<Option<(String, Vec<u8>)>, AuthorPhotosDataError> {
    // Filter `letter` rows in SQL as well as via the schema CHECK constraint
    // so a malformed row (e.g. left over from a future migration drift)
    // can never accidentally serve `letter` bytes as a real image. Belt +
    // suspenders alongside the table-level invariant.
    let row: Option<(String, Option<String>, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT source, mime, bytes
           FROM author_photos
          WHERE author_id = ? AND source <> 'letter'",
    )
    .bind(author_id)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((_, Some(mime), Some(bytes))) if !bytes.is_empty() => Ok(Some((mime, bytes))),
        _ => Ok(None),
    }
}

/// Look up just the cascade-state metadata (source + fetched_at) for an
/// author. Used by the resolver to decide whether to skip resolution — a
/// `'letter'` row prevents re-querying Open Library until an admin clears it
/// via `delete_author_photo`.
pub async fn author_photo_status(
    pool: &SqlitePool,
    author_id: i64,
) -> Result<Option<(AuthorPhotoSource, String)>, AuthorPhotosDataError> {
    let row: Option<(String, String)> =
        sqlx::query_as("SELECT source, fetched_at FROM author_photos WHERE author_id = ?")
            .bind(author_id)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(s, t)| AuthorPhotoSource::parse(&s).map(|src| (src, t))))
}

/// Upsert a photo row. Replaces any existing row (PRIMARY KEY conflict on
/// `author_id`). `bytes` / `mime` are `None` for `'letter'` negative-cache
/// markers; `url` is `None` for manual uploads.
pub async fn upsert_author_photo(
    pool: &SqlitePool,
    author_id: i64,
    source: AuthorPhotoSource,
    url: Option<&str>,
    mime: Option<&str>,
    bytes: Option<&[u8]>,
) -> Result<(), AuthorPhotosDataError> {
    sqlx::query(
        "INSERT INTO author_photos (author_id, source, url, mime, bytes, fetched_at)
              VALUES (?, ?, ?, ?, ?, datetime('now'))
         ON CONFLICT(author_id) DO UPDATE SET
              source     = excluded.source,
              url        = excluded.url,
              mime       = excluded.mime,
              bytes      = excluded.bytes,
              fetched_at = excluded.fetched_at",
    )
    .bind(author_id)
    .bind(source.as_str())
    .bind(url)
    .bind(mime)
    .bind(bytes)
    .execute(pool)
    .await?;
    Ok(())
}

/// Drop the cache row for an author so the next page view re-queues
/// resolution. Used by admin DELETE.
pub async fn delete_author_photo(
    pool: &SqlitePool,
    author_id: i64,
) -> Result<(), AuthorPhotosDataError> {
    sqlx::query("DELETE FROM author_photos WHERE author_id = ?")
        .bind(author_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Delete an author by id and prevent reindex from re-creating the row.
///
/// One transaction: look up the name (for the blocklist insert) and
/// affected book ids (for the post-commit FTS refresh), drop the link
/// rows, drop the `authors` row, then `INSERT OR IGNORE` the name into
/// `ignored_authors` so the next `indexer::reindex` does not silently
/// re-create the row via `resolve_or_insert_author`.
///
/// Returns the number of books that were un-linked (used by the admin
/// confirmation modal to show "this affects N books"). Returns `Ok(0)`
/// without touching the blocklist if `author_id` does not exist — a
/// stale tab firing a second Delete must not leak a ghost row into
/// `ignored_authors`.
///
/// FTS refresh runs best-effort *after* commit, mirroring
/// [`crate::metadata_overrides::upsert_metadata_overrides`]: a stale FTS
/// row is fixed on the next reindex, but a refresh failure must not undo
/// the admin's intent.
pub async fn delete_author(
    pool: &SqlitePool,
    author_id: i64,
) -> Result<u64, AuthorPhotosDataError> {
    let mut tx = pool.begin().await?;

    let Some(name): Option<String> = sqlx::query_scalar("SELECT name FROM authors WHERE id = ?")
        .bind(author_id)
        .fetch_optional(&mut *tx)
        .await?
    else {
        return Ok(0);
    };

    // Snapshot the affected book UUIDs *inside* the transaction by
    // joining link → books. Doing the lookup as a uuid join (one bind
    // parameter) instead of post-commit `WHERE id IN (?, ?, …)` keeps
    // the post-commit FTS refresh from hitting SQLite's 999 bind-
    // parameter cap on authors linked to >999 books — without that
    // cap a giant-author delete would silently skip its FTS rebuild
    // and leave the index stale.
    let affected_uuids: Vec<String> = sqlx::query_scalar(
        "SELECT b.uuid FROM books b
         JOIN books_authors_link l ON l.book = b.id
         WHERE l.author = ?",
    )
    .bind(author_id)
    .fetch_all(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM books_authors_link WHERE author = ?")
        .bind(author_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query("DELETE FROM authors WHERE id = ?")
        .bind(author_id)
        .execute(&mut *tx)
        .await?;

    sqlx::query("INSERT OR IGNORE INTO ignored_authors(name) VALUES (?)")
        .bind(&name)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    // Batch-rebuild FTS in at most 2 write statements per 100-UUID chunk
    // (one DELETE + one INSERT) so an author linked to N books never produces
    // N×2 sequential write-lock acquisitions.
    if let Err(e) = rebuild_fts_for_books_batch(pool, &affected_uuids).await {
        tracing::warn!(
            author_id,
            book_count = affected_uuids.len(),
            error = %e,
            "books_fts batch rebuild after delete_author failed"
        );
    }

    Ok(affected_uuids.len() as u64)
}

#[cfg(test)]
mod tests;
