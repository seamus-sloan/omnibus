//! Page aggregation for the stats page: the Pages read tile (its total, its
//! per-day series and its coverage disclosure), the reading rate beside it, and
//! the length-distribution chart. All of them resolve a book's length through
//! the one ladder in [`book_pages_source`] — every input is persisted at index
//! time, so no EPUB or archive is opened at query time.
//!
//! They are scoped to **three different questions**, and conflating any two of
//! them is the bug this module has already had once:
//!
//! * [`pages_read`] is ground *covered* in the window, so it windows on the
//!   forward-progress ledger (migration `0083`). Summing the length of the
//!   books finished in the window — what it used to do — reported an em-dash to
//!   a reader who finished nothing and attributed a whole book to whichever
//!   window its status flip landed in, which is the Finished tile weighted by
//!   length, not pages read.
//! * [`length_buckets`] is about books *finished*, so it windows on completion
//!   events. That is its subject, not an oversight.
//! * [`pages_per_hour`] is a reading *speed*, so it pairs the books finished in
//!   the window with those books' lifetime reading seconds — both sides have to
//!   describe the same books, which is why it can't be `pages_read` over
//!   `reading_seconds`. It is the one aggregate that divides, so it alone drops
//!   a book the ladder resolves to zero pages.

use omnibus_shared::{LengthBucket, PagesReadDetail, TrendPoint};
use sqlx::{Row, SqlitePool};

use crate::progress::SLOT_SECS;

use super::{calendar, StatsError, FINISHED_EVENTS};

/// Words per printed page, the standard prose estimate (the same ballpark
/// self-publishing/KDP page-count calculators use for a 6x9 trade
/// paperback). Not exact — the stored word count is itself a spine-text
/// estimate — but documented and consistent, which is what the tile's
/// "clearly-labelled estimated-page count" asks for.
const WORDS_PER_PAGE: f64 = 275.0;

/// The length buckets, in order, as `(label, exclusive upper bound in pages)`.
/// The last one is open-ended.
///
/// These are StoryGraph's thresholds, and matching them is deliberate: they
/// map onto how readers already talk about length — a quick read, an ordinary
/// novel, a doorstopper. Finer buckets would split the middle into bars nobody
/// reads any differently.
///
/// Labels and bounds sit in one array so a boundary and the bar naming it stay
/// next to each other under any later edit.
const LENGTH_BUCKETS: [(&str, Option<i64>); 3] = [
    ("Under 300", Some(300)),
    ("300\u{2013}499", Some(500)),
    ("500+", None),
];

/// The bucket for a book no rung of the ladder can measure. It exists rather
/// than being dropped because an audiobook has no page analogue and a
/// not-yet-backfilled EPUB has a NULL `word_count` — folding either into the
/// shortest bucket would report a library as shorter than it is.
const UNKNOWN_LABEL: &str = "Unknown";

/// One `(uuid, pages)` row per book: **the** length ladder, in one place.
///
/// The order is the whole point:
/// 1. `metadata_overrides.overrides -> '$.print_pages'` — a real print-edition
///    count from the metadata lookup, so it outranks every estimate.
/// 2. `books.page_count` — exact, but only ever set for a CBZ comic (migration
///    `0063`). An image-page count is the right answer for a comic and
///    meaningless anywhere else, which is why it can't lead.
/// 3. `books.word_count / WORDS_PER_PAGE` (migration `0049`) — the EPUB spine
///    estimate, the weakest rung and so the last.
///
/// `pages` is NULL for a book none of the three can measure. That is
/// "not measured", never zero, and callers must keep the distinction:
/// `SUM` skips it, and the distribution buckets it as [`UNKNOWN_LABEL`].
///
/// Takes no binds — it is library-wide, and every caller narrows it by joining
/// its own set of book uuids.
pub(super) fn book_pages_source() -> String {
    format!(
        "SELECT b.uuid AS uuid,
                COALESCE(
                    CAST(json_extract(mo.overrides, '$.print_pages') AS INTEGER),
                    b.page_count,
                    -- CAST to REAL first: `word_count` is an INTEGER column, and
                    -- an integer-looking divisor makes this integer division,
                    -- which truncates instead of rounding (a 260-word book
                    -- would come out zero pages).
                    CAST(ROUND(CAST(b.word_count AS REAL) / {WORDS_PER_PAGE:.1}) AS INTEGER)
                ) AS pages
         FROM books b
         LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid"
    )
}

/// The distinct live books a user finished in the window — the shared scope of
/// all three aggregates here, and the same completion definition as
/// `compute::finished_count` (a 100% journal entry or an explicit read-status
/// `finished`, on a book that still exists). A book finished twice counts once.
/// Bind order is `user_id, user_id, start`.
pub(super) fn finished_in_window() -> String {
    format!(
        "SELECT DISTINCT f.book_uuid AS uuid
         FROM ({FINISHED_EVENTS}) f
         JOIN books b ON b.uuid = f.book_uuid
         WHERE f.finished_at >= ?"
    )
}

/// Every forward-progress gain one reader has on record, resolved to a day on
/// **their** calendar: one `(day, uuid, percent_gained)` row per bucket.
///
/// Unions the ledger's two generations, which is the whole reason this exists.
/// `reading_progress_slots` (migration `0095`) keys on a quarter-hour, so its
/// day is computed here against `offset_minutes` and follows the reader.
/// `reading_progress_daily` (`0083`) stored a UTC day string and kept no
/// instant, so there is nothing to re-bucket it from — those rows contribute
/// their stored day verbatim and are the reason days around the cutover can sit
/// a few hours out. The old table is frozen, so its share shrinks to nothing as
/// its days age out of every window.
///
/// Bind order is `user_id, user_id`.
fn ledger_days(offset_minutes: i64) -> String {
    let slot_day = calendar::local_day(&format!("(s.slot * {SLOT_SECS})"), offset_minutes);
    format!(
        "SELECT {slot_day} AS day,
                s.book_uuid AS uuid,
                s.percent_gained AS percent_gained
           FROM reading_progress_slots s WHERE s.user_id = ?
         UNION ALL
         SELECT d.day, d.book_uuid, d.percent_gained
           FROM reading_progress_daily d WHERE d.user_id = ?"
    )
}

/// The window's ledger gains joined to their book's resolved length: one
/// `(day, uuid, percent_gained, pages)` row per bucket. `pages` is NULL for a
/// book no rung measures, and the callers each decide what to do with those
/// rather than the join dropping them silently — the total excludes them, the
/// coverage counts them.
///
/// Bind order is `user_id, user_id, start`. `start` is a unix second compared as
/// the day it falls in **on the reader's calendar**, so it lines up with the day
/// labels the union produces; comparing it as a UTC day would include a
/// leading extra day for every reader east of UTC.
pub(super) fn ledger_in_window(offset_minutes: i64) -> String {
    format!(
        "SELECT g.day AS day,
                g.uuid AS uuid,
                g.percent_gained AS percent_gained,
                p.pages AS pages
         FROM ({}) g
         JOIN ({}) p ON p.uuid = g.uuid
         WHERE g.day >= {}",
        ledger_days(offset_minutes),
        book_pages_source(),
        calendar::local_day("?", offset_minutes)
    )
}

/// Pages read in the window: for every book, the fraction of it covered inside
/// the window times its resolved length, summed.
///
/// `None` when no book read in the window resolves a length at all — nothing
/// read, or everything read is not-yet-backfilled. That is the em-dash, and it
/// is **not** the same as an audio-only window; [`pages_detail`] is what tells
/// the two apart.
///
/// Rounding happens once, on the total, rather than per book: a reader who
/// turned a few pages in each of six books should not lose a page to rounding
/// six times over.
pub(super) async fn pages_read(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    offset_minutes: i64,
) -> Result<Option<i64>, StatsError> {
    // `SUM` over zero rows is SQL NULL, which maps straight to the tile's
    // em-dash `None` — an unmeasured book must not read as a zero-page one.
    let sql = format!(
        "SELECT CAST(ROUND(SUM(CAST(w.percent_gained AS REAL) * w.pages) / 100.0) AS INTEGER)
         FROM ({}) w
         WHERE w.pages IS NOT NULL",
        ledger_in_window(offset_minutes)
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await?)
}

/// One `(book_uuid, secs)` row per book the user has ever read, carrying that
/// book's **lifetime** reading seconds. Lifetime rather than windowed on
/// purpose: a book begun in March and finished in April would otherwise pair
/// April's whole length with April's hours alone and print an absurd rate.
///
/// `reading_sessions` only — pages per hour is a reading-speed figure, and
/// folding `listening_sessions` in would measure the narrator instead.
/// One bind: `user_id`.
const BOOK_READING_SECS: &str = "\
    SELECT book_uuid, SUM(seconds_read) AS secs FROM reading_sessions \
        WHERE user_id = ? GROUP BY book_uuid";

/// Estimated pages read per hour over the window: a seconds-weighted mean,
/// not a mean of per-book rates.
///
/// Both sides describe **the same books** — every distinct live book finished
/// in the window that resolves a length *and* carries recorded reading time.
/// A book with length but no reading seconds contributes neither, since its
/// words with nobody's hours behind them would inflate the rate; a book with
/// hours but no resolvable length is dropped for the mirror-image reason.
///
/// `None` when that set is empty, which drives the same em-dash empty state
/// [`pages_read`] does. Books read partly in audio over-report here — their
/// listening time is excluded from the denominator by design — which is why
/// the surfaces label this an estimate.
///
/// A book resolving to **zero** pages is dropped, where [`pages_read`] sums it
/// harmlessly. Two ways to get one: `estimate_word_count` yields `Some(0)` for
/// an EPUB whose spine loads but strips to no words (image-only or
/// fixed-layout), and the ladder's own rounding sends anything under half a
/// page there too. Either way it donates its hours while contributing no
/// pages, dragging the rate down — and as the only qualifying book it would
/// print a "0 pages an hour" that is exactly the claim the empty state exists
/// to avoid. `pages_read` and [`length_buckets`] keep their existing treatment
/// of a stored zero; only a figure with a denominator is hurt by one.
pub(super) async fn pages_per_hour(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Option<f64>, StatsError> {
    let sql = format!(
        "SELECT SUM(p.pages) AS pages, SUM(t.secs) AS secs
         FROM ({}) fin
         JOIN ({}) p ON p.uuid = fin.uuid
         JOIN ({BOOK_READING_SECS}) t ON t.book_uuid = fin.uuid
         WHERE p.pages > 0 AND t.secs > 0",
        finished_in_window(),
        book_pages_source()
    );
    let row = sqlx::query(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .bind(user_id)
        .fetch_one(pool)
        .await?;
    Ok(hourly_rate(row.get("pages"), row.get("secs")))
}

/// Pages per hour from the summed pair, or `None` when either side is absent
/// (`SUM` over zero rows is SQL NULL) or the seconds are non-positive. Split
/// out so the guard against a zero denominator is one expression rather than
/// something the SQL's `t.secs > 0` filter is silently trusted for.
fn hourly_rate(pages: Option<i64>, secs: Option<i64>) -> Option<f64> {
    let (pages, secs) = (pages?, secs?);
    if secs <= 0 {
        return None;
    }
    // Page and second totals sit far below f64's 2^52 exact-integer range.
    #[allow(clippy::cast_precision_loss)]
    let rate = pages as f64 / (secs as f64 / 3600.0);
    Some(rate)
}

/// [`pages_read`] over an explicit `[start, end]` slice, for the drill-in's
/// vs-previous-period delta. Zero rather than `None` when the baseline window
/// measured nothing — the delta treats "no baseline" as new, and a missing
/// baseline and an empty one are the same thing to it.
///
/// The slice is half-open like every other bounded aggregate, but resolved a
/// **day** at a time: the last day counted is the one `end - 1s` falls in. That
/// keeps a partial boundary day in (the current window it is compared against
/// always includes a partial today, so dropping it would bias every comparison
/// downward) while keeping a boundary that lands exactly on midnight out.
///
/// The distinction is not academic. `compute::prev_window_from` clamps `end` to
/// the current period's start once the elapsed slice fills the previous period,
/// which happens on the last days of most months — and `date(end)` then names
/// day one of the *current* window, so an inclusive comparison would count that
/// day into its own baseline and report a 0% delta against it.
pub(super) async fn pages_read_bounded(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    end: i64,
    offset_minutes: i64,
) -> Result<i64, StatsError> {
    let sql = format!(
        "SELECT COALESCE(
                    CAST(ROUND(SUM(CAST(w.percent_gained AS REAL) * w.pages) / 100.0) AS INTEGER),
                    0)
         FROM ({}) w
         WHERE w.pages IS NOT NULL AND w.day <= {}",
        ledger_in_window(offset_minutes),
        calendar::local_day("? - 1", offset_minutes)
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .bind(end)
        .fetch_one(pool)
        .await?)
}

/// Pages covered on one UTC `YYYY-MM-DD`, for the daily pages goal.
///
/// Zero rather than `None` when nothing resolves, which is the one place this
/// module deliberately departs from [`pages_read`]: that function's `None` is
/// the tile's em-dash, telling "nothing measurable" apart from "nothing read".
/// A goal has no such state — a reader who has not opened a book today is at
/// zero of their target, and an em-dash there would read as the goal being
/// broken rather than as the day being young.
///
/// `day` is on the reader's own calendar, and so is the bucketing this compares
/// it against — which is the point of migration `0093`. A gain the reader made
/// at 21:00 counts toward the day they made it on, not the one UTC had rolled
/// over into. Bind order is `user_id, user_id, day`.
pub(super) async fn pages_read_on_day(
    pool: &SqlitePool,
    user_id: i64,
    day: &str,
    offset_minutes: i64,
) -> Result<i64, StatsError> {
    let sql = format!(
        "SELECT COALESCE(
                    CAST(ROUND(SUM(CAST(g.percent_gained AS REAL) * p.pages) / 100.0) AS INTEGER),
                    0)
         FROM ({}) g
         JOIN ({}) p ON p.uuid = g.uuid
         WHERE g.day = ? AND p.pages IS NOT NULL",
        ledger_days(offset_minutes),
        book_pages_source()
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(day)
        .fetch_one(pool)
        .await?)
}

/// Everything the Pages tile needs beyond its headline: the per-day series
/// behind it, which books it could and could not measure, and the day the
/// ledger started.
pub(super) async fn pages_detail(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    offset_minutes: i64,
) -> Result<PagesReadDetail, StatsError> {
    let (measured_books, unmeasured_books) =
        pages_book_counts(pool, user_id, start, offset_minutes).await?;
    let since_day = ledger_epoch(pool).await?;
    // Resolved here rather than from the `StatsRange`, because the range does
    // not answer it: a Year window in the calendar year after the epoch is
    // fully covered, and a Week window in the days just after it is not. Both
    // sides are `YYYY-MM-DD`, so a lexicographic compare is a date compare.
    let window_predates_ledger = match since_day.as_deref() {
        Some(epoch) => start_day(pool, start, offset_minutes).await?.as_str() < epoch,
        None => false,
    };
    Ok(PagesReadDetail {
        since_day,
        measured_books,
        unmeasured_books,
        audio_books: audio_books(pool, user_id, start).await?,
        daily: pages_daily(pool, user_id, start, offset_minutes).await?,
        window_predates_ledger,
    })
}

/// The day a window's `start` unix second falls in on the reader's calendar, as
/// `YYYY-MM-DD`. Resolved in SQLite so it uses the same calendar the union in
/// [`ledger_days`] labels its buckets with.
async fn start_day(
    pool: &SqlitePool,
    start: i64,
    offset_minutes: i64,
) -> Result<String, StatsError> {
    let sql = format!("SELECT {}", calendar::local_day("?", offset_minutes));
    Ok(sqlx::query_scalar(&sql).bind(start).fetch_one(pool).await?)
}

/// Pages per day within the window on the reader's calendar, active days only,
/// ascending.
async fn pages_daily(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    offset_minutes: i64,
) -> Result<Vec<TrendPoint>, StatsError> {
    let sql = format!(
        "SELECT w.day AS day,
                CAST(ROUND(SUM(CAST(w.percent_gained AS REAL) * w.pages) / 100.0) AS INTEGER)
                    AS pages
         FROM ({}) w
         WHERE w.pages IS NOT NULL
         GROUP BY w.day
         ORDER BY w.day",
        ledger_in_window(offset_minutes)
    );
    let rows = sqlx::query(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .fetch_all(pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(|r| TrendPoint {
            label: r.get("day"),
            // Day totals sit far below f64's exact-integer range.
            #[allow(clippy::cast_precision_loss)]
            value: r.get::<i64, _>("pages") as f64,
        })
        .collect())
}

/// `(measured, unmeasured)` distinct books read in the window — the second is
/// real reading the total cannot include, and a tile that never says so
/// understates itself without admitting it.
async fn pages_book_counts(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    offset_minutes: i64,
) -> Result<(i64, i64), StatsError> {
    let sql = format!(
        "SELECT COUNT(DISTINCT CASE WHEN w.pages IS NOT NULL THEN w.uuid END) AS measured,
                COUNT(DISTINCT CASE WHEN w.pages IS NULL     THEN w.uuid END) AS unmeasured
         FROM ({}) w",
        ledger_in_window(offset_minutes)
    );
    let row = sqlx::query(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await?;
    Ok((row.get("measured"), row.get("unmeasured")))
}

/// Distinct live books listened to in the window. Audiobooks contribute no
/// pages — there is no page analogue — but a window of nothing but listening
/// must not read as a window of nothing.
async fn audio_books(pool: &SqlitePool, user_id: i64, start: i64) -> Result<i64, StatsError> {
    Ok(sqlx::query_scalar(
        "SELECT COUNT(DISTINCT ls.book_uuid)
         FROM listening_sessions ls
         JOIN books b ON b.uuid = ls.book_uuid
         WHERE ls.user_id = ? AND ls.started_at >= ?",
    )
    .bind(user_id)
    .bind(start)
    .fetch_one(pool)
    .await?)
}

/// The day the forward-progress ledger began recording, straight from the
/// `settings` row migration `0083` wrote. Read here rather than through
/// `db::progress` so the stats layer keeps its own error type.
async fn ledger_epoch(pool: &SqlitePool) -> Result<Option<String>, StatsError> {
    Ok(
        sqlx::query_scalar("SELECT value FROM settings WHERE key = 'pages_ledger_epoch'")
            .fetch_optional(pool)
            .await?,
    )
}

/// How the books finished in the window are distributed across
/// [`LENGTH_BUCKETS`], plus the unknown bucket.
///
/// Every bucket comes back, zeros included — a distribution with missing bars
/// reads as a different distribution. An all-zero result means nothing was
/// finished in the window; the surfaces render their empty state off the total
/// rather than drawing flat bars.
pub(super) async fn length_buckets(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Vec<LengthBucket>, StatsError> {
    let sql = format!(
        "SELECT {} AS bucket, COUNT(*) AS books
         FROM ({}) fin
         JOIN ({}) p ON p.uuid = fin.uuid
         GROUP BY bucket",
        bucket_case_sql("p.pages"),
        finished_in_window(),
        book_pages_source()
    );
    let rows = sqlx::query(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .fetch_all(pool)
        .await?;

    let mut counts = vec![0_i64; LENGTH_BUCKETS.len() + 1];
    for row in rows {
        let bucket: i64 = row.get("bucket");
        if let Some(slot) = usize::try_from(bucket).ok().and_then(|i| counts.get_mut(i)) {
            *slot = row.get("books");
        }
    }

    Ok(LENGTH_BUCKETS
        .iter()
        .map(|(label, _)| *label)
        .chain(std::iter::once(UNKNOWN_LABEL))
        .zip(counts)
        .map(|(label, books)| LengthBucket {
            label: label.to_string(),
            books,
        })
        .collect())
}

/// The `CASE` mapping a resolved page count to its [`LENGTH_BUCKETS`] index,
/// with the unknown bucket last. Generated from the same array the labels come
/// from, so a boundary and its bar can never disagree.
fn bucket_case_sql(pages_col: &str) -> String {
    let unknown = LENGTH_BUCKETS.len();
    // The fall-through is whichever bucket has no upper bound — *derived*, not
    // assumed to be the last entry. Hardcoding the tail index would keep
    // working right up until someone reorders the array, and then silently
    // file every long book under whatever landed there.
    let open = LENGTH_BUCKETS
        .iter()
        .position(|(_, upper)| upper.is_none())
        .unwrap_or(unknown.saturating_sub(1));
    let mut sql = format!("CASE WHEN {pages_col} IS NULL THEN {unknown} ");
    for (i, (_, upper)) in LENGTH_BUCKETS.iter().enumerate() {
        if let Some(bound) = upper {
            sql.push_str(&format!("WHEN {pages_col} < {bound} THEN {i} "));
        }
    }
    sql.push_str(&format!("ELSE {open} END"));
    sql
}

#[cfg(test)]
mod tests;
