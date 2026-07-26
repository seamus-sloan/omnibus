//! Read-only book listing that feeds native Kobo wireless sync
//! (`/kobo/v1/library/sync`). Scoped to the requesting user's shelves that are
//! flagged `sync_to_kobo` — never the whole library.

use serde::Serialize;
use sqlx::{Row, SqlitePool};

use crate::resolve_canonical_book_uuid;

/// SELECT list shared by [`sync_books`] and [`book_for_sync`]. `b` is the
/// `books` alias; the correlated subquery pulls the lowest-`position` author.
const SELECT_COLS: &str = "b.uuid,
                COALESCE(b.title, '') AS title,
                COALESCE((
                    SELECT a.name
                    FROM books_authors_link bal
                    JOIN authors a ON a.id = bal.author
                    WHERE bal.book = b.id
                    ORDER BY bal.position ASC, a.name ASC
                    LIMIT 1
                ), '') AS author,
                CAST(COALESCE(b.last_modified, 0) AS INTEGER) AS last_modified_epoch";

/// One book shaped for the Kobo sync endpoint: durable uuid, display title, a
/// best-effort author line, and the last-modified epoch that drives the
/// device's metadata-freshness check.
#[derive(Debug, Clone, Serialize)]
pub struct KoboBookRow {
    pub uuid: String,
    pub title: String,
    pub author: String,
    pub last_modified_epoch: i64,
}

#[derive(Debug, thiserror::Error)]
pub enum KoboError {
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::shelves::ShelfError> for KoboError {
    fn from(e: crate::shelves::ShelfError) -> Self {
        match e {
            crate::shelves::ShelfError::Sqlx(inner) => Self::Sqlx(inner),
            // `kobo_synced_book_uuids` only reads rows and translates stored
            // rules; the remaining variants are mutation/validation failures it
            // cannot produce. Fold rather than widen `KoboError` with arms no
            // caller can branch on.
            other => Self::Sqlx(sqlx::Error::Protocol(other.to_string())),
        }
    }
}

impl From<crate::books::BooksError> for KoboError {
    fn from(e: crate::books::BooksError) -> Self {
        match e {
            crate::books::BooksError::Db(inner) => Self::Sqlx(inner),
            // `resolve_canonical_book_uuid` never decodes overrides JSON, so
            // this arm is unreachable in practice; fold it into a decode error
            // rather than panicking, mirroring `ratings`/`read_status`.
            crate::books::BooksError::OverridesJson(inner) => {
                Self::Sqlx(sqlx::Error::Decode(Box::new(inner)))
            }
        }
    }
}

/// The books `user_id`'s Kobo devices may sync, newest-modified first: the
/// union of membership across that user's shelves flagged `sync_to_kobo`.
///
/// Sync is **never whole-library** (#924) — a user with no opted-in shelf gets
/// an empty set, which is the correct answer, not a degenerate one. The author
/// is the lowest-`position` entry in `books_authors_link` (empty when a book
/// has none — Kobo tolerates a blank author).
///
/// Scoped through shelf ownership, so a device token can only ever reach books
/// its own user opted in.
pub async fn sync_books(pool: &SqlitePool, user_id: i64) -> Result<Vec<KoboBookRow>, KoboError> {
    let uuids = crate::shelves::kobo_synced_book_uuids(pool, user_id).await?;
    if uuids.is_empty() {
        return Ok(Vec::new());
    }
    // Bind the uuid set rather than interpolating it: the values are
    // server-derived, but a parameterized IN list keeps the query plan on the
    // `books.uuid` UNIQUE index and never risks a quoting bug.
    let placeholders = std::iter::repeat_n("?", uuids.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = format!(
        "SELECT {SELECT_COLS}
         FROM books b
         WHERE b.uuid IS NOT NULL AND b.uuid != ''
           AND b.uuid IN ({placeholders})
         ORDER BY b.last_modified DESC, b.id DESC"
    );
    let mut q = sqlx::query(&sql);
    for uuid in &uuids {
        q = q.bind(uuid);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows.iter().map(row_to_book).collect())
}

/// Fetch a single book for the Kobo `library/<uuid>/metadata` endpoint,
/// resolving `uuid` through the merged-uuid ledger so a format-merged id still
/// finds the surviving book. `Ok(None)` for an unknown/ghosted uuid.
pub async fn book_for_sync(
    pool: &SqlitePool,
    uuid: &str,
) -> Result<Option<KoboBookRow>, KoboError> {
    let Some(canonical) = resolve_canonical_book_uuid(pool, uuid).await? else {
        return Ok(None);
    };
    let sql = format!("SELECT {SELECT_COLS} FROM books b WHERE b.uuid = ?");
    let row = sqlx::query(&sql)
        .bind(&canonical)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(row_to_book))
}

fn row_to_book(row: &sqlx::sqlite::SqliteRow) -> KoboBookRow {
    KoboBookRow {
        uuid: row.get("uuid"),
        title: row.get("title"),
        author: row.get("author"),
        last_modified_epoch: row.get("last_modified_epoch"),
    }
}

#[cfg(test)]
mod tests;
