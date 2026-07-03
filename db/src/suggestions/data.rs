//! Cache CRUD + the pure [`decide`] de-duplication state machine for F3.3
//! suggestions. Two tables back this (migration 0033): `book_suggestion_state`
//! holds one TTL/`pending`/`resolved`/`empty` marker per source book, and
//! `book_suggestions` holds its 0..N cached result rows. Read by [`cascade`]
//! and the suggestions RPC/REST handlers.

use sqlx::SqlitePool;

/// Result cache TTL: a book is re-resolved only after 30 days.
pub const SUGGESTIONS_TTL_SECS: i64 = 30 * 24 * 60 * 60;

/// Debounce window for an in-flight `pending` marker. A second viewer landing
/// within this window reads `pending` and does **not** post another task; a
/// `pending` marker older than this is treated as a dead resolution and
/// re-posted.
pub const PENDING_DEBOUNCE_SECS: i64 = 15 * 60;

/// Errors returned by the suggestions data layer. Single transparent `Db`
/// variant per the `02-error-handling` boundary rule — never leak
/// `sqlx::Error` across the module boundary.
#[derive(Debug, thiserror::Error)]
pub enum SuggestionsDataError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// State of a per-book cache marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SuggestionState {
    /// A resolution is enqueued / in flight.
    Pending,
    /// Results are cached (`book_suggestions` holds 0..N rows).
    Resolved,
    /// Sticky negative — Hardcover matched nothing usable.
    Empty,
}

impl SuggestionState {
    /// Database string for this state.
    pub fn as_str(&self) -> &'static str {
        match self {
            SuggestionState::Pending => "pending",
            SuggestionState::Resolved => "resolved",
            SuggestionState::Empty => "empty",
        }
    }

    /// Parse a database string; `None` for unrecognised values.
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(Self::Pending),
            "resolved" => Some(Self::Resolved),
            "empty" => Some(Self::Empty),
            _ => None,
        }
    }
}

/// What the read path should do for a book, derived purely from its cache
/// marker and the current time. `serve` = return whatever rows are cached;
/// `enqueue` = post a (re)resolution task. The RPC marks `pending` before it
/// posts so concurrent callers can't each enqueue.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheDecision {
    pub serve: bool,
    pub enqueue: bool,
}

/// Pure cache state machine. `marker` is the `(state, fetched_at)` pair from
/// [`suggestion_state`] (`None` = no row yet); `now` is unix-seconds.
///
/// This is the single place that guarantees a page refresh or a burst of
/// concurrent viewers never re-hits Hardcover while a result is fresh: a 30-day
/// TTL result cache, plus a `pending` debounce so concurrent first-viewers
/// collapse to one posted resolution task.
pub fn decide(marker: Option<(SuggestionState, i64)>, now: i64) -> CacheDecision {
    match marker {
        // No marker: first viewer. Nothing to serve; kick off a resolution.
        None => CacheDecision {
            serve: false,
            enqueue: true,
        },
        // In flight: serve nothing, and only re-post if the marker is old
        // enough that the prior task almost certainly died.
        Some((SuggestionState::Pending, at)) => CacheDecision {
            serve: false,
            enqueue: now.saturating_sub(at) > PENDING_DEBOUNCE_SECS,
        },
        // Resolved/empty: serve the cached rows; refresh only past the TTL.
        Some((SuggestionState::Resolved | SuggestionState::Empty, at)) => CacheDecision {
            serve: true,
            enqueue: now.saturating_sub(at) > SUGGESTIONS_TTL_SECS,
        },
    }
}

/// A cached suggestion row as served to the API layer (cover bytes excluded —
/// those stream through a separate endpoint). `has_cover` tells the frontend
/// whether to point a cover `<img>` at the cover endpoint or fall back to the
/// typographic plate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CachedSuggestion {
    pub rank: i64,
    pub hardcover_id: i64,
    pub hardcover_slug: Option<String>,
    pub title: String,
    pub author: String,
    pub list_count: i64,
    pub has_cover: bool,
}

/// A freshly-resolved suggestion to persist. `rank` is assigned by insertion
/// order in [`replace_suggestions`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewSuggestion {
    pub hardcover_id: i64,
    pub hardcover_slug: Option<String>,
    pub title: String,
    pub author: String,
    pub list_count: i64,
    pub cover_mime: Option<String>,
    pub cover_bytes: Option<Vec<u8>>,
}

/// Read the cache marker `(state, fetched_at)` for a book, or `None` if it has
/// never been resolved.
pub async fn suggestion_state(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<Option<(SuggestionState, i64)>, SuggestionsDataError> {
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT state, fetched_at FROM book_suggestion_state WHERE book_uuid = ?")
            .bind(book_uuid)
            .fetch_optional(pool)
            .await?;
    Ok(row.and_then(|(s, at)| SuggestionState::parse(&s).map(|st| (st, at))))
}

/// Raw projection row for [`get_suggestions`]:
/// `(rank, hardcover_id, hardcover_slug, title, author, list_count, has_cover)`.
type SuggestionProjection = (i64, i64, Option<String>, String, String, i64, bool);

/// Fetch the cached suggestion rows for a book, ordered by rank.
pub async fn get_suggestions(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<Vec<CachedSuggestion>, SuggestionsDataError> {
    let rows: Vec<SuggestionProjection> = sqlx::query_as(
        // `has_cover` guards on length too, not just NOT NULL: the cover
        // endpoint (`get_suggestion_cover`) rejects empty blobs, so a
        // zero-length row must not advertise a cover URL that would 404.
        // Belt + suspenders alongside the cascade's empty-bytes filter.
        "SELECT rank, hardcover_id, hardcover_slug, title, author, list_count,
                (cover_bytes IS NOT NULL AND length(cover_bytes) > 0) AS has_cover
           FROM book_suggestions
          WHERE book_uuid = ?
          ORDER BY rank",
    )
    .bind(book_uuid)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(rank, hardcover_id, hardcover_slug, title, author, list_count, has_cover)| {
                CachedSuggestion {
                    rank,
                    hardcover_id,
                    hardcover_slug,
                    title,
                    author,
                    list_count,
                    has_cover,
                }
            },
        )
        .collect())
}

/// Fetch one cached cover's `(mime, bytes)`, or `None` when the rank has no
/// stored image. Served by the cover endpoint.
pub async fn get_suggestion_cover(
    pool: &SqlitePool,
    book_uuid: &str,
    rank: i64,
) -> Result<Option<(String, Vec<u8>)>, SuggestionsDataError> {
    let row: Option<(Option<String>, Option<Vec<u8>>)> = sqlx::query_as(
        "SELECT cover_mime, cover_bytes FROM book_suggestions WHERE book_uuid = ? AND rank = ?",
    )
    .bind(book_uuid)
    .bind(rank)
    .fetch_optional(pool)
    .await?;
    match row {
        Some((Some(mime), Some(bytes))) if !bytes.is_empty() => Ok(Some((mime, bytes))),
        _ => Ok(None),
    }
}

/// Mark a book `pending` (upsert state + reset the `fetched_at` clock). Called
/// by the RPC immediately *before* posting a resolution task so a burst of
/// concurrent first-viewers collapses to a single posted job.
pub async fn mark_pending(pool: &SqlitePool, book_uuid: &str) -> Result<(), SuggestionsDataError> {
    sqlx::query(
        "INSERT INTO book_suggestion_state (book_uuid, state, fetched_at)
              VALUES (?, 'pending', strftime('%s','now'))
         ON CONFLICT(book_uuid) DO UPDATE SET
              state      = 'pending',
              fetched_at = strftime('%s','now')",
    )
    .bind(book_uuid)
    .execute(pool)
    .await?;
    Ok(())
}

/// Replace a book's cached suggestions and stamp its terminal marker, in one
/// transaction. `rows` are written at ranks `0..rows.len()`; an empty `rows`
/// slice with `state = Empty` records the sticky-negative marker.
pub async fn replace_suggestions(
    pool: &SqlitePool,
    book_uuid: &str,
    rows: &[NewSuggestion],
    state: SuggestionState,
) -> Result<(), SuggestionsDataError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM book_suggestions WHERE book_uuid = ?")
        .bind(book_uuid)
        .execute(&mut *tx)
        .await?;
    for (rank, s) in rows.iter().enumerate() {
        sqlx::query(
            "INSERT INTO book_suggestions
                 (book_uuid, rank, hardcover_id, hardcover_slug, title, author,
                  list_count, cover_mime, cover_bytes)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(book_uuid)
        .bind(rank as i64)
        .bind(s.hardcover_id)
        .bind(s.hardcover_slug.as_deref())
        .bind(&s.title)
        .bind(&s.author)
        .bind(s.list_count)
        .bind(s.cover_mime.as_deref())
        .bind(s.cover_bytes.as_deref())
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "INSERT INTO book_suggestion_state (book_uuid, state, fetched_at)
              VALUES (?, ?, strftime('%s','now'))
         ON CONFLICT(book_uuid) DO UPDATE SET
              state      = excluded.state,
              fetched_at = excluded.fetched_at",
    )
    .bind(book_uuid)
    .bind(state.as_str())
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

/// Drop a book's cached suggestions and marker so the next view re-resolves.
/// Used by tests and any future admin "refresh suggestions" affordance.
pub async fn delete_suggestions(
    pool: &SqlitePool,
    book_uuid: &str,
) -> Result<(), SuggestionsDataError> {
    let mut tx = pool.begin().await?;
    sqlx::query("DELETE FROM book_suggestions WHERE book_uuid = ?")
        .bind(book_uuid)
        .execute(&mut *tx)
        .await?;
    sqlx::query("DELETE FROM book_suggestion_state WHERE book_uuid = ?")
        .bind(book_uuid)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}
