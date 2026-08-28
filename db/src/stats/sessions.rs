//! Reverse-chronological reading-session log — the per-sitting detail behind
//! the aggregates the rest of `db::stats` reports. Reads the same
//! `reading_sessions` / `listening_sessions` tables, stitched into sittings by
//! [`sessionize`], for one user and optionally one book.

use omnibus_shared::{SessionCursor, SessionFormat, SessionLogEntry, SessionLogPage};
use sqlx::{Row, SqlitePool};

use super::{sessionize, StatsError};

#[cfg(test)]
mod tests;

/// Page size when a caller names none.
pub const SESSION_LOG_DEFAULT_LIMIT: i64 = 25;

/// Hard ceiling on a page. The log grows for as long as the reader does, so
/// the bound is the server's to set, not the caller's — a client asking for
/// more gets this many.
pub const SESSION_LOG_MAX_LIMIT: i64 = 100;

/// One `(book_uuid, started_at, ended_at, secs, is_audio)` union of every
/// checkpoint row a user recorded, in either table.
///
/// `book_uuid` is read straight off the row rather than resolved through the
/// merge ledger, which is what keeps this log and the aggregates beside it
/// counting the same sittings. Two writers guarantee the column is already
/// canonical: `progress::session::record_session_tx` resolves the reported
/// uuid before it inserts, and `merge::transaction`'s `RETARGET_TABLES`
/// carries both session tables onto the surviving book. Resolving again per
/// row would only ever change rows those two cannot produce — at the cost of
/// making the scoped filter below unable to use the `(user_id, book_uuid)`
/// index, and of counting sittings `book_insights` does not.
///
/// Bind order is `user_id, user_id`.
const USER_SESSION_ROWS: &str = "\
    SELECT book_uuid, started_at, ended_at, seconds_read AS secs, 0 AS is_audio \
      FROM reading_sessions WHERE user_id = ? \
    UNION ALL \
    SELECT book_uuid, started_at, ended_at, seconds_listened, 1 \
      FROM listening_sessions WHERE user_id = ?";

/// The page query. `book_scoped` adds the canonical-uuid filter, `cursored`
/// the keyset predicate; both are threaded as binds by [`session_log`], which
/// owns the matching bind order.
fn page_sql(book_scoped: bool, cursored: bool) -> String {
    let rows = if book_scoped {
        format!("SELECT * FROM ({USER_SESSION_ROWS}) WHERE book_uuid = ?")
    } else {
        USER_SESSION_ROWS.to_string()
    };
    let sittings = sessionize::stitched(&rows);
    let min_secs = sessionize::MIN_SITTING_SECS;
    // Keyset, not offset: a session landing mid-scroll shifts every later
    // page under an offset, which repeats or skips a sitting. Ordering and
    // predicate name the same `(started_at, book_uuid)` pair, so a tie
    // between two books that started in the same second still advances.
    let keyset = if cursored {
        "AND (s.started_at < ? OR (s.started_at = ? AND s.book_uuid < ?)) "
    } else {
        ""
    };
    format!(
        "SELECT s.book_uuid AS book_uuid, s.started_at AS started_at, \
                s.ended_at AS ended_at, s.secs AS secs, \
                s.min_audio AS min_audio, s.max_audio AS max_audio, \
                COALESCE(b.title, 'Untitled') AS title \
         FROM ({sittings}) s \
         LEFT JOIN books b ON b.uuid = s.book_uuid \
         WHERE s.secs >= {min_secs} {keyset}\
         ORDER BY s.started_at DESC, s.book_uuid DESC \
         LIMIT ?"
    )
}

/// One page of a user's session log, newest sitting first.
///
/// `book_uuid` scopes the log to a single book and is resolved to the
/// canonical `books.uuid` first, mirroring [`super::book_insights`] — so a
/// link into a book that was later merged away still finds the sittings
/// recorded under the surviving one. An unresolvable uuid yields an empty
/// page rather than an error: the caller's next move is the same empty state
/// either way.
///
/// `before` continues a previous page; `None` starts at the newest sitting.
/// `limit` is clamped into `1..=`[`SESSION_LOG_MAX_LIMIT`].
///
/// Entries are stitched sittings of at least [`sessionize::MIN_SITTING_SECS`]
/// — the same population [`super::book_insights`] counts as Pickups, so the
/// per-book log's length matches the number rendered beside it. A glance is
/// not hidden data so much as not a sitting; its seconds still reach every
/// total.
pub async fn session_log(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: Option<&str>,
    before: Option<&SessionCursor>,
    limit: i64,
) -> Result<SessionLogPage, StatsError> {
    let limit = limit.clamp(1, SESSION_LOG_MAX_LIMIT);
    let canonical = match book_uuid {
        Some(uuid) => match crate::books::resolve_canonical_book_uuid(pool, uuid).await? {
            Some(resolved) => Some(resolved),
            None => return Ok(SessionLogPage::default()),
        },
        None => None,
    };

    let sql = page_sql(canonical.is_some(), before.is_some());
    let mut query = sqlx::query(&sql).bind(user_id).bind(user_id);
    if let Some(uuid) = &canonical {
        query = query.bind(uuid);
    }
    if let Some(cursor) = before {
        query = query
            .bind(cursor.started_at)
            .bind(cursor.started_at)
            .bind(&cursor.book_uuid);
    }
    // One row past the page so "is there more" is answered by what came back
    // rather than by whether the page filled — which would hand out a cursor
    // to an empty final page on an exact multiple.
    let rows = query.bind(limit + 1).fetch_all(pool).await?;

    let has_more = i64::try_from(rows.len()).unwrap_or(i64::MAX) > limit;
    let mut entries: Vec<SessionLogEntry> = rows
        .into_iter()
        .map(|r| {
            let min_audio: i64 = r.get("min_audio");
            let max_audio: i64 = r.get("max_audio");
            SessionLogEntry {
                book_uuid: r.get("book_uuid"),
                title: r.get("title"),
                format: session_format(min_audio, max_audio),
                started_at: r.get("started_at"),
                ended_at: r.get("ended_at"),
                seconds: r.get("secs"),
            }
        })
        .collect();
    entries.truncate(usize::try_from(limit).unwrap_or(usize::MAX));

    let next_before = has_more
        .then(|| entries.last().map(|e| e.cursor().encode()))
        .flatten();
    Ok(SessionLogPage {
        entries,
        next_before,
    })
}

/// Which tables fed a stitched sitting, from the stitch's `is_audio` bounds.
fn session_format(min_audio: i64, max_audio: i64) -> SessionFormat {
    match (min_audio, max_audio) {
        (0, 0) => SessionFormat::Reading,
        (0, _) => SessionFormat::Mixed,
        _ => SessionFormat::Listening,
    }
}
