//! Book-length aggregation for the stats page: the Est. pages tile's total and
//! the length-distribution chart beside it. Both resolve a book's length
//! through the one ladder in [`book_pages_source`] — every input is persisted
//! at index time, so no EPUB or archive is opened at query time.

use omnibus_shared::LengthBucket;
use sqlx::{Row, SqlitePool};

use super::{StatsError, FINISHED_EVENTS};

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
/// both aggregates here, and the same completion definition as
/// `compute::finished_count` (a 100% journal entry or an explicit read-status
/// `finished`, on a book that still exists). A book finished twice counts once.
/// Bind order is `user_id, user_id, start`.
fn finished_in_window() -> String {
    format!(
        "SELECT DISTINCT f.book_uuid AS uuid
         FROM ({FINISHED_EVENTS}) f
         JOIN books b ON b.uuid = f.book_uuid
         WHERE f.finished_at >= ?"
    )
}

/// Estimated pages read in the window: the resolved length of every distinct
/// book finished within it, summed. `None` when no finished book in the window
/// resolves a length at all — none finished, or every finished one is
/// audio-only / not-yet-backfilled.
pub(super) async fn pages_read(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
) -> Result<Option<i64>, StatsError> {
    // `SUM` over zero rows is SQL NULL, which maps straight to the tile's
    // em-dash `None` — an unmeasured book must not read as a zero-page one.
    let sql = format!(
        "SELECT SUM(p.pages)
         FROM ({}) fin
         JOIN ({}) p ON p.uuid = fin.uuid
         WHERE p.pages IS NOT NULL",
        finished_in_window(),
        book_pages_source()
    );
    Ok(sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await?)
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
