//! Book listing that feeds native Kobo wireless sync
//! (`/kobo/v1/library/sync`). Scoped to the requesting user's shelves that are
//! flagged `sync_to_kobo` — never the whole library. [`delta`] turns that set
//! into the per-device add/change/remove diff the protocol expects.

use omnibus_shared::ReadStatus;
use serde::Serialize;
use sqlx::{Row, SqlitePool};

use crate::resolve_canonical_book_uuid;

pub mod annotations;
pub mod delta;

pub use delta::{clear_snapshot, record_synced, sync_delta, SyncChange, SyncDelta};

/// SELECT list shared by [`sync_books`] and [`book_for_sync`]. `b` is the
/// `books` alias; the correlated subquery pulls the lowest-`position` author.
const SELECT_COLS: &str = "b.id AS id,
                b.uuid,
                COALESCE(b.title, '') AS title,
                COALESCE((
                    SELECT a.name
                    FROM books_authors_link bal
                    JOIN authors a ON a.id = bal.author
                    WHERE bal.book = b.id
                    ORDER BY bal.position ASC, a.name ASC
                    LIMIT 1
                ), '') AS author,
                CAST(COALESCE(b.last_modified, 0) AS INTEGER) AS last_modified_epoch,
                COALESCE((
                    SELECT bf.size_bytes
                    FROM book_files bf
                    WHERE bf.book_id = b.id AND bf.format = 'EPUB'
                    ORDER BY bf.ordinal ASC
                    LIMIT 1
                ), (
                    SELECT bf.size_bytes
                    FROM book_files bf
                    WHERE bf.book_id = b.id AND bf.format = 'CBZ'
                    ORDER BY bf.ordinal ASC
                    LIMIT 1
                ), 0) AS download_size_bytes,
                EXISTS(
                    SELECT 1 FROM book_files bf
                    WHERE bf.book_id = b.id AND bf.format = 'EPUB'
                ) AS has_epub";

/// One book shaped for the Kobo sync endpoint: durable uuid, display title, a
/// best-effort author line, and the last-modified epoch that drives the
/// device's metadata-freshness check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct KoboBookRow {
    /// `books.id` — lets sync-out reach the kepub cache and source file for
    /// span derivation without a second lookup.
    pub id: i64,
    pub uuid: String,
    pub title: String,
    pub author: String,
    pub last_modified_epoch: i64,
    /// Size of the file the download route would serve: the lowest-ordinal
    /// EPUB, else the lowest-ordinal CBZ for a comic-only book, `0` when the
    /// book has neither. Advertised on the download entitlement — a
    /// best-effort figure (the served KEPUB differs slightly), consumed only
    /// by device-side progress UI, never for validation.
    pub download_size_bytes: i64,
    /// Whether the book has any EPUB file — decides the advertised download
    /// format (`KEPUB` vs `CBZ`), mirroring the branch `download` takes.
    pub has_epub: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum KoboError {
    /// Resolving the opted-in shelf membership failed for a non-DB reason —
    /// in practice a stored `kind`/`visibility`/rule that no longer parses.
    /// Kept distinct from [`Self::Sqlx`] so the real cause survives into the
    /// log instead of being disguised as a protocol error.
    #[error("shelf membership lookup failed: {0}")]
    Shelf(String),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

impl From<crate::shelves::ShelfError> for KoboError {
    fn from(e: crate::shelves::ShelfError) -> Self {
        match e {
            crate::shelves::ShelfError::Sqlx(inner) => Self::Sqlx(inner),
            other => Self::Shelf(other.to_string()),
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

impl From<crate::metadata_overrides::MetadataOverridesError> for KoboError {
    fn from(e: crate::metadata_overrides::MetadataOverridesError) -> Self {
        match e {
            crate::metadata_overrides::MetadataOverridesError::Db(inner) => Self::Sqlx(inner),
            crate::metadata_overrides::MetadataOverridesError::Serialization(inner) => {
                Self::Sqlx(sqlx::Error::Decode(Box::new(inner)))
            }
            crate::metadata_overrides::MetadataOverridesError::Io(inner) => {
                Self::Sqlx(sqlx::Error::Io(inner))
            }
            // Bulk-write-only variants that can't arise on the sync read
            // path; folded with their message preserved rather than
            // panicking (mirrors `BooksError`'s `From<MetadataOverridesError>`).
            other @ (crate::metadata_overrides::MetadataOverridesError::BookNotFound(_)
            | crate::metadata_overrides::MetadataOverridesError::TooManyValues {
                ..
            }) => Self::Sqlx(sqlx::Error::Protocol(other.to_string())),
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
    // Bind the uuids rather than interpolating them: the values are
    // server-derived, but a parameterized IN list keeps the query plan on the
    // `books.uuid` UNIQUE index and never risks a quoting bug.
    //
    // Chunked at 900 because a single `IN (...)` would otherwise blow SQLite's
    // 999 bound-parameter ceiling — the exact failure the "no cap" goal exists
    // to avoid, just relocated from the protocol to the driver. Mirrors
    // `resolve_book_ids_bulk` in `books/get.rs` (chunked at 499 there because
    // it binds each uuid twice).
    let mut rows = Vec::with_capacity(uuids.len());
    for chunk in uuids.chunks(900) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(",");
        let sql = format!(
            "SELECT {SELECT_COLS}
             FROM books b
             WHERE b.uuid IS NOT NULL AND b.uuid != ''
               AND b.uuid IN ({placeholders})"
        );
        let mut q = sqlx::query(&sql);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        rows.extend(q.fetch_all(pool).await?.iter().map(row_to_book));
    }
    // Ordering moves out of SQL because the result set is now assembled across
    // chunks; sorting here keeps newest-modified-first over the whole set
    // rather than within each chunk.
    rows.sort_by_key(|r| std::cmp::Reverse(r.last_modified_epoch));
    apply_title_author_overrides(pool, &mut rows).await?;
    Ok(rows)
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
    let Some(mut book) = row.as_ref().map(row_to_book) else {
        return Ok(None);
    };
    apply_title_author_overrides(pool, std::slice::from_mut(&mut book)).await?;
    Ok(Some(book))
}

/// Overlay each row's title/author with its saved `metadata_overrides` (the
/// same [`crate::metadata_overrides::apply_overrides`] merge `db::get_book`
/// runs for every other book-detail surface), gated by the owning scan
/// root's configured source precedence (#972). A no-op for rows whose uuid
/// has no override row.
async fn apply_title_author_overrides(
    pool: &SqlitePool,
    rows: &mut [KoboBookRow],
) -> Result<(), KoboError> {
    let uuids: Vec<String> = rows.iter().map(|r| r.uuid.clone()).collect();
    let overrides = crate::metadata_overrides::load_overrides_bulk(pool, &uuids).await?;
    if overrides.is_empty() {
        return Ok(());
    }
    let overridden_uuids: Vec<String> = overrides.keys().cloned().collect();
    let precedence = crate::settings::metadata_precedence_by_uuid(pool, &overridden_uuids).await?;
    for row in rows.iter_mut() {
        let Some((ov, has_cover_override)) = overrides.get(&row.uuid) else {
            continue;
        };
        let prec = precedence
            .get(&row.uuid)
            .cloned()
            .unwrap_or_else(|| omnibus_shared::DEFAULT_METADATA_PRECEDENCE.to_vec());
        // A throwaway `EbookMetadata` seeded with this row's current
        // title/author lets `apply_overrides` do the real merge (title
        // replace, precedence gate, creators-list replace) instead of
        // re-deriving those rules here.
        let mut book = omnibus_shared::EbookMetadata {
            title: Some(row.title.clone()),
            creators: if row.author.is_empty() {
                Vec::new()
            } else {
                vec![omnibus_shared::Contributor {
                    name: row.author.clone(),
                    ..Default::default()
                }]
            },
            ..Default::default()
        };
        crate::metadata_overrides::apply_overrides(
            &mut book,
            &row.uuid,
            ov,
            *has_cover_override,
            &prec,
        );
        if let Some(title) = book.title {
            row.title = title;
        }
        row.author = book
            .creators
            .first()
            .map(|c| c.name.clone())
            .unwrap_or_default();
    }
    Ok(())
}

fn row_to_book(row: &sqlx::sqlite::SqliteRow) -> KoboBookRow {
    KoboBookRow {
        id: row.get("id"),
        uuid: row.get("uuid"),
        title: row.get("title"),
        author: row.get("author"),
        last_modified_epoch: row.get("last_modified_epoch"),
        download_size_bytes: row.get("download_size_bytes"),
        has_epub: row.get("has_epub"),
    }
}

/// One book's per-user reading state, as the Kobo protocol wants it: a read
/// status, plus the position halves a device can act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KoboBookState {
    pub status: ReadStatus,
    pub percent: Option<i64>,
    /// Opaque `CurrentBookmark.Location` JSON, echoed back verbatim.
    pub kobo_location: Option<String>,
    /// The stored CFI, when the freshest position came from the web/iOS
    /// reader. Sync-out derives a KoboSpan from it when `kobo_location`
    /// is absent; it is never emitted to the device directly.
    pub epub_cfi: Option<String>,
    /// Newest of the read-status and progress timestamps — the value the
    /// per-device delta compares against its snapshot to decide whether a
    /// device's state is stale.
    pub state_updated_at: i64,
    /// The progress row's own event time (`COALESCE(client_updated_at,
    /// updated_at)`), 0 when the book has no progress row. This — not
    /// [`Self::state_updated_at`], which is MAXed with the read-status
    /// clock — is the optimistic token for
    /// `progress::attach_derived_kobo_location`.
    pub progress_updated_at: i64,
    /// The read-status row's own event time, 0 when the book has no status
    /// row. Kept separate from [`Self::progress_updated_at`] because the
    /// device arbitrates `StatusInfo` and `CurrentBookmark` independently,
    /// each against its own clock — the MAXed value would claim the status
    /// moved every time only the position did.
    pub status_updated_at: i64,
    /// The device's last-reported `Statistics`, echoed back untouched
    /// (#1653). Absent from [`Self::state_updated_at`] on purpose: a
    /// stats-only change is no reason to re-push a book the device agrees
    /// with, and the block carries its own clock regardless.
    pub statistics: Option<crate::progress::KoboStatistics>,
}

/// Per-user reading state for `uuids`, keyed by book uuid.
///
/// Books with neither a read-status nor a progress row are simply absent —
/// the caller treats a miss as "unread, no position", which is what a fresh
/// book is. Chunked at 900 for the same SQLite bound-parameter ceiling
/// [`sync_books`] works around (each uuid binds twice here, so the chunk is
/// halved to 450).
pub async fn reading_state_for(
    pool: &SqlitePool,
    user_id: i64,
    uuids: &[String],
) -> Result<std::collections::HashMap<String, KoboBookState>, KoboError> {
    let mut out = std::collections::HashMap::with_capacity(uuids.len());
    if uuids.is_empty() {
        return Ok(out);
    }
    for chunk in uuids.chunks(450) {
        for row in fetch_reading_state_chunk(pool, user_id, chunk).await? {
            let (uuid, state) = row_to_kobo_state(&row)?;
            out.insert(uuid, state);
        }
    }
    Ok(out)
}

/// The per-chunk union-and-join query text: `book_read_status` and
/// `reading_progress` can each have a row independently, so the two key
/// sets are unioned and both sides are left-joined onto it (SQLite has no
/// `FULL OUTER JOIN`). `placeholders` is shared by the two `IN (...)`
/// clauses that scope the union to this chunk's uuids.
fn reading_state_sql(placeholders: &str) -> String {
    format!(
        "WITH keys(book_uuid) AS (
             SELECT book_uuid FROM book_read_status
              WHERE user_id = ? AND book_uuid IN ({placeholders})
             UNION
             SELECT book_uuid FROM reading_progress
              WHERE user_id = ? AND format = 'epub' AND book_uuid IN ({placeholders})
         )
         SELECT k.book_uuid,
                rs.status AS status,
                rp.progress_percent AS progress_percent,
                rp.kobo_location AS kobo_location,
                rp.epub_cfi AS epub_cfi,
                rp.kobo_spent_reading_minutes AS kobo_spent_reading_minutes,
                rp.kobo_remaining_time_minutes AS kobo_remaining_time_minutes,
                rp.kobo_statistics_updated_at AS kobo_statistics_updated_at,
                COALESCE(rp.client_updated_at, rp.updated_at, 0)
                    AS progress_updated_at,
                COALESCE(rs.updated_at, 0) AS status_updated_at,
                MAX(
                    COALESCE(rs.updated_at, 0),
                    COALESCE(rp.client_updated_at, rp.updated_at, 0)
                ) AS state_updated_at
           FROM keys k
           LEFT JOIN book_read_status rs
                  ON rs.book_uuid = k.book_uuid AND rs.user_id = ?
           LEFT JOIN reading_progress rp
                  ON rp.book_uuid = k.book_uuid AND rp.user_id = ? AND rp.format = 'epub'"
    )
}

/// Run [`reading_state_sql`] for one chunk of uuids, binding `user_id` and
/// the chunk into each of the query's four parameter slots.
async fn fetch_reading_state_chunk(
    pool: &SqlitePool,
    user_id: i64,
    chunk: &[String],
) -> Result<Vec<sqlx::sqlite::SqliteRow>, KoboError> {
    let placeholders = std::iter::repeat_n("?", chunk.len())
        .collect::<Vec<_>>()
        .join(",");
    let sql = reading_state_sql(&placeholders);
    let mut q = sqlx::query(&sql).bind(user_id);
    for uuid in chunk {
        q = q.bind(uuid);
    }
    q = q.bind(user_id);
    for uuid in chunk {
        q = q.bind(uuid);
    }
    q = q.bind(user_id).bind(user_id);
    Ok(q.fetch_all(pool).await?)
}

/// Parse one joined row into its `(uuid, state)` pair.
fn row_to_kobo_state(row: &sqlx::sqlite::SqliteRow) -> Result<(String, KoboBookState), KoboError> {
    let uuid: String = row.try_get("book_uuid")?;
    let status = row
        .try_get::<Option<String>, _>("status")?
        .map(|s| ReadStatus::from_db(&s))
        .unwrap_or_default();
    let statistics = crate::progress::KoboStatistics {
        spent_reading_minutes: row.try_get("kobo_spent_reading_minutes")?,
        remaining_time_minutes: row.try_get("kobo_remaining_time_minutes")?,
        updated_at: row.try_get("kobo_statistics_updated_at")?,
    };
    let state = KoboBookState {
        status,
        // A row of NULLs is "no device ever reported": collapse it so
        // sync-out omits the block rather than emitting empties.
        statistics: (!statistics.is_empty()).then_some(statistics),
        percent: row.try_get::<Option<i64>, _>("progress_percent")?,
        kobo_location: row.try_get::<Option<String>, _>("kobo_location")?,
        epub_cfi: row.try_get::<Option<String>, _>("epub_cfi")?,
        state_updated_at: row.try_get::<i64, _>("state_updated_at")?,
        progress_updated_at: row.try_get::<i64, _>("progress_updated_at")?,
        status_updated_at: row.try_get::<i64, _>("status_updated_at")?,
    };
    Ok((uuid, state))
}

#[cfg(test)]
mod tests;
