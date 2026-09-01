//! The window's single most-X figures — longest and shortest book, biggest
//! reading day, longest sit, fastest read. Every input is already persisted;
//! each figure is one ranked query over a table the rest of `db::stats`
//! already aggregates.
//!
//! The two day-shaped figures — biggest day, fastest read — cut their days on
//! `offset_minutes` via `calendar`, because they render beside the heatmap and
//! the pages tile that do the same.

use omnibus_shared::{BookSuperlative, DayActivity, Superlatives, FASTEST_READ_MIN_SECS};
use sqlx::{Row, SqlitePool};

use super::compute::{SESSION_ROWS, USER_SESSION_ROWS};
use super::{calendar, pages, sessionize, StatsError};

/// The author join every book-naming superlative shares: position-0 creator,
/// left-joined so a book with no author link still wins its category.
const AUTHOR_JOIN: &str = "\
    LEFT JOIN books_authors_link bal ON bal.book = b.id AND bal.position = 0 \
    LEFT JOIN authors a ON a.id = bal.author";

/// The three columns every book-naming superlative selects, minus its own
/// `value`. `COALESCE` on the title mirrors `compute::finished_books` — an
/// untitled row still has to name itself.
const BOOK_COLUMNS: &str = "\
    b.uuid AS uuid, COALESCE(b.title, 'Untitled') AS title, a.name AS author";

/// Every superlative for one user's window. Runs the ranked queries and
/// applies the one cross-figure rule: a "shortest book" that says nothing
/// next to the longest is dropped rather than rendered (see
/// [`drop_degenerate_shortest`]).
pub(super) async fn superlatives(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    offset_minutes: i64,
) -> Result<Superlatives, StatsError> {
    let longest_book = extreme_book(pool, user_id, start, Extreme::Longest).await?;
    let shortest_book = extreme_book(pool, user_id, start, Extreme::Shortest).await?;
    let biggest_day = biggest_day(pool, user_id, start, offset_minutes).await?;
    let longest_sit = longest_sit(pool, user_id, start).await?;
    let fastest_read = fastest_read(pool, user_id, start, offset_minutes).await?;

    Ok(Superlatives {
        shortest_book: drop_degenerate_shortest(longest_book.as_ref(), shortest_book),
        longest_book,
        biggest_day,
        longest_sit,
        fastest_read,
    })
}

/// Which end of the length ordering a query wants.
#[derive(Clone, Copy)]
enum Extreme {
    Longest,
    Shortest,
}

impl Extreme {
    fn direction(self) -> &'static str {
        match self {
            Extreme::Longest => "DESC",
            Extreme::Shortest => "ASC",
        }
    }
}

/// The longest or shortest book finished in the window, measured by the
/// shared length ladder. Books the ladder can't measure are excluded rather
/// than sorted as zero — an audiobook is not the shortest book of the year.
/// `> 0` rather than `IS NOT NULL` is what enforces that: an image-only EPUB
/// loads its spine and strips to zero words, so the ladder hands back a real
/// `0` that would otherwise win "shortest" outright.
/// Ties break on the *rendered* title — the `COALESCE`d alias rather than
/// `b.title` — so an untitled book sorts where the reader sees it instead of
/// ahead of everything on a NULL.
async fn extreme_book(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    extreme: Extreme,
) -> Result<Option<BookSuperlative>, StatsError> {
    let sql = format!(
        "SELECT {BOOK_COLUMNS}, p.pages AS value
         FROM ({}) fin
         JOIN books b ON b.uuid = fin.uuid
         JOIN ({}) p ON p.uuid = fin.uuid
         {AUTHOR_JOIN}
         WHERE p.pages > 0
         ORDER BY p.pages {}, title ASC
         LIMIT 1",
        pages::finished_in_window(),
        pages::book_pages_source(),
        extreme.direction()
    );
    let row = sqlx::query(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(book_superlative))
}

/// Suppress a "shortest book" that only restates the longest one.
///
/// Two ways it can: naming the *same* book (the window finished exactly one
/// measurable book), or carrying the same page count (every finished book is
/// the same length). Both render as a range the window doesn't have, which is
/// the noise the all-`Option` shape exists to avoid.
fn drop_degenerate_shortest(
    longest: Option<&BookSuperlative>,
    shortest: Option<BookSuperlative>,
) -> Option<BookSuperlative> {
    let (long, short) = (longest?, shortest?);
    (long.book_uuid != short.book_uuid && long.value != short.value).then_some(short)
}

/// The single busiest day in the window, reading and listening together — a
/// `MAX` over the same per-day rollup `compute::heatmap` builds, on the same
/// [`calendar::local_day`] buckets. Ties break to the earliest day.
///
/// The two ride one `StatsSummary` onto one page, so a `MAX` cut in UTC
/// headlines a day the heatmap beside it draws as empty.
///
/// A day under [`sessionize::MIN_SITTING_SECS`] is no day at all: the surfaces
/// render this figure in whole minutes, so a window whose only activity was a
/// 45-second glance would otherwise headline "Biggest day — 0 m". The same
/// floor [`longest_sit`] uses, so the card's two time-shaped rows agree on
/// what counts.
async fn biggest_day(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    offset_minutes: i64,
) -> Result<Option<DayActivity>, StatsError> {
    let sql = format!(
        "SELECT {} AS day, SUM(secs) AS seconds
         FROM ({SESSION_ROWS})
         GROUP BY day HAVING seconds >= {}
         ORDER BY seconds DESC, day ASC LIMIT 1",
        calendar::local_day("started_at", offset_minutes),
        sessionize::MIN_SITTING_SECS
    );
    let row = sqlx::query(&sql)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .bind(start)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| DayActivity {
        day: r.get("day"),
        seconds: r.get("seconds"),
    }))
}

/// The longest single sitting in the window — the user-wide twin of
/// `book::book_insights`'s `longest_seconds`, over the same stitched
/// checkpoint rows so this reports how long the reader sat rather than how
/// long a client waited between flushes. Glances under
/// [`sessionize::MIN_SITTING_SECS`] are excluded, as everywhere else a
/// sitting is counted. Ties break to the earliest sitting.
async fn longest_sit(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Option<BookSuperlative>, StatsError> {
    let sql = format!(
        "SELECT {BOOK_COLUMNS}, s.secs AS value
         FROM ({}) s
         JOIN books b ON b.uuid = s.book_uuid
         {AUTHOR_JOIN}
         WHERE s.secs >= {}
         ORDER BY s.secs DESC, s.started_at ASC
         LIMIT 1",
        sessionize::stitched(SESSION_ROWS),
        sessionize::MIN_SITTING_SECS
    );
    let row = sqlx::query(&sql)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .bind(start)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(book_superlative))
}

/// The book finished in the window in the fewest days from its **first
/// recorded session**, in whole days on the reader's calendar, with a same-day
/// read reported as one rather than zero.
///
/// Both ends go through [`calendar::local_day_number`]. The shift is common to
/// them and usually cancels — but not when it carries only one end across a
/// midnight, which is precisely when the reader counts the days differently.
///
/// Three deliberate choices, each of which the naive version gets wrong:
///
/// - **Lifetime sessions, not the window's.** The read started when it
///   started; clipping to the window would report a book begun in March and
///   finished in April as an April sprint.
/// - **The earliest in-window completion**, so re-marking an old book
///   finished can't stretch a read that already ended.
/// - **A [`FASTEST_READ_MIN_SECS`] floor**, because the figure measures
///   *tracked* reading. A book read on a Kobo and marked finished here
///   carries one stray checkpoint, and without the floor that book wins every
///   time. The floor narrows the lie; it does not remove it, which is why the
///   surfaces present this as a lower bound.
///
/// A book whose only recorded sessions post-date its completion (finished
/// elsewhere, re-read here) is dropped rather than reported as a negative
/// span. Ties break on the rendered title, as above.
async fn fastest_read(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    offset_minutes: i64,
) -> Result<Option<BookSuperlative>, StatsError> {
    let finished_day = calendar::local_day_number("f.finished_at", offset_minutes);
    let first_day = calendar::local_day_number("s.first_at", offset_minutes);
    let sql = format!(
        "SELECT {BOOK_COLUMNS},
                MAX(1, {finished_day} - {first_day}) AS value
         FROM (
             SELECT book_uuid, MIN(finished_at) AS finished_at
             FROM ({}) WHERE finished_at >= ? GROUP BY book_uuid
         ) f
         JOIN books b ON b.uuid = f.book_uuid
         JOIN (
             SELECT book_uuid, MIN(started_at) AS first_at, SUM(secs) AS secs
             FROM ({USER_SESSION_ROWS}) GROUP BY book_uuid
         ) s ON s.book_uuid = f.book_uuid
         {AUTHOR_JOIN}
         WHERE s.secs >= {FASTEST_READ_MIN_SECS} AND s.first_at <= f.finished_at
         ORDER BY value ASC, title ASC
         LIMIT 1",
        super::FINISHED_EVENTS
    );
    let row = sqlx::query(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.as_ref().map(book_superlative))
}

/// Read the four columns every book-naming superlative selects off a row.
fn book_superlative(row: &sqlx::sqlite::SqliteRow) -> BookSuperlative {
    BookSuperlative {
        book_uuid: row.get("uuid"),
        title: row.get("title"),
        author: row.get("author"),
        value: row.get("value"),
    }
}

#[cfg(test)]
mod tests;
