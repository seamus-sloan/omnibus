//! Unit tests for the calendar expression builders. Each runs its expression
//! through SQLite rather than asserting on the generated string — the string is
//! an implementation detail, and a test that pinned it would pass while the SQL
//! it produces was wrong.

use crate::init_db;

use super::*;

/// 2023-11-14 22:13:20 UTC — late enough in the UTC day that a western offset
/// pulls it back a day and an eastern one pushes it forward.
const T0: i64 = 1_700_000_000;

async fn scalar(sql: &str) -> String {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query_scalar(&format!("SELECT {sql}"))
        .fetch_one(&pool)
        .await
        .unwrap()
}

async fn scalar_i64(sql: &str) -> i64 {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query_scalar(&format!("SELECT {sql}"))
        .fetch_one(&pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn local_day_shifts_the_date_across_the_boundary_in_both_directions() {
    let utc = scalar(&local_day(&T0.to_string(), 0)).await;
    let tokyo = scalar(&local_day(&T0.to_string(), 540)).await;
    let la = scalar(&local_day(&T0.to_string(), -420)).await;

    assert_eq!(utc, "2023-11-14");
    // 22:13 UTC is already the 15th in Tokyo and still the 14th in Los Angeles.
    assert_eq!(tokyo, "2023-11-15");
    assert_eq!(la, "2023-11-14");
}

#[tokio::test]
async fn local_day_handles_a_quarter_hour_zone() {
    // UTC+05:45 (Kathmandu) is why the ledger buckets at a quarter-hour rather
    // than hourly — a whole-hour grid cannot place this boundary.
    let day = scalar(&local_day("1699999200", 345)).await;

    assert_eq!(day, "2023-11-15");
}

#[tokio::test]
async fn local_day_number_agrees_with_local_day() {
    let dnum = scalar_i64(&local_day_number(&T0.to_string(), 540)).await;
    let via_dnum = scalar(&format!("date({dnum} * 86400, 'unixepoch')")).await;
    let direct = scalar(&local_day(&T0.to_string(), 540)).await;

    // The streak counts in day numbers and the heatmap labels in date strings;
    // if these two ever disagreed a streak would be drawn against days the grid
    // does not show.
    assert_eq!(via_dnum, direct);
}

#[tokio::test]
async fn local_day_number_floors_for_a_pre_epoch_timestamp() {
    // A device with a badly wrong clock can file one. Truncating toward zero
    // would map both -1s and +1s onto day 0 and weld two days into one active
    // day; flooring keeps them apart.
    let before = scalar_i64(&local_day_number("-1", 0)).await;
    let after = scalar_i64(&local_day_number("1", 0)).await;

    assert_eq!(before, -1);
    assert_eq!(after, 0);
}

#[tokio::test]
async fn local_month_shifts_across_a_month_boundary() {
    // 2023-11-30 23:30 UTC is already December in Tokyo.
    let utc = scalar(&local_month("1701387000", 0)).await;
    let tokyo = scalar(&local_month("1701387000", 540)).await;

    assert_eq!(utc, "2023-11-30"[..7].to_string());
    assert_eq!(tokyo, "2023-12");
}

#[tokio::test]
async fn window_start_expr_lands_on_the_readers_midnight() {
    for offset in [0, 540, -420, 345] {
        let start = scalar_i64(&format!(
            "CAST({} AS INTEGER)",
            window_start_expr(StatsRange::Month, offset)
        ))
        .await;
        // Whatever the offset, the boundary must be midnight *on the reader's
        // clock* — the point of the trailing shift back to UTC.
        let local = scalar(&local_day(&start.to_string(), offset)).await;
        let time = scalar(&format!(
            "strftime('%H:%M', {} + {}, 'unixepoch')",
            start,
            offset * 60
        ))
        .await;

        assert_eq!(time, "00:00", "offset {offset} did not land on midnight");
        assert!(local.ends_with("-01"), "offset {offset} gave {local}");
    }
}

#[tokio::test]
async fn window_start_expr_is_zero_for_all_time() {
    let start = scalar_i64(&format!(
        "CAST({} AS INTEGER)",
        window_start_expr(StatsRange::AllTime, -420)
    ))
    .await;

    assert_eq!(start, 0, "an unbounded window has no offset to apply");
}

#[tokio::test]
async fn prev_window_start_expr_precedes_the_current_one() {
    for range in [StatsRange::Week, StatsRange::Month, StatsRange::Year] {
        let cur = scalar_i64(&format!(
            "CAST({} AS INTEGER)",
            window_start_expr(range, -420)
        ))
        .await;
        let prev = scalar_i64(&format!(
            "CAST({} AS INTEGER)",
            prev_window_start_expr(range, -420).unwrap()
        ))
        .await;

        assert!(prev < cur, "{range:?}: {prev} should precede {cur}");
    }
}

#[tokio::test]
async fn prev_window_start_expr_is_absent_for_all_time() {
    assert!(prev_window_start_expr(StatsRange::AllTime, 0).is_none());
}

#[tokio::test]
async fn today_expr_yields_a_matching_day_and_day_number() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let row = sqlx::query(&today_expr(540))
        .fetch_one(&pool)
        .await
        .unwrap();
    let day: String = sqlx::Row::get(&row, "day");
    let dnum: i64 = sqlx::Row::get(&row, "dnum");

    let from_dnum: String = sqlx::query_scalar("SELECT date(? * 86400, 'unixepoch')")
        .bind(dnum)
        .fetch_one(&pool)
        .await
        .unwrap();

    // Both come out of one statement precisely so they cannot straddle midnight
    // and describe different days.
    assert_eq!(day, from_dnum);
}

#[tokio::test]
async fn window_start_expr_stays_on_a_midnight_for_an_absurd_offset() {
    // Only reachable from a caller that skipped `resolve_offset_minutes`, and
    // the failure would not look like one: the shift and the modifier are two
    // expressions in one query, so bounding either alone leaves a window edge
    // that is nobody's midnight. `bounded` is shared precisely so they can't.
    let start = scalar_i64(&format!(
        "CAST({} AS INTEGER)",
        window_start_expr(StatsRange::Month, i64::MAX)
    ))
    .await;
    let time = scalar(&format!(
        "strftime('%H:%M', {start} + {}, 'unixepoch')",
        SessionReport::UTC_OFFSET_MAX_MINUTES * 60
    ))
    .await;

    assert_eq!(time, "00:00");
}

#[tokio::test]
async fn shift_parenthesises_a_negative_offset() {
    // `x - -25200` would lex as a `--` line comment and swallow the rest of the
    // statement; the parens are load-bearing, not cosmetic.
    let secs = scalar_i64(&format!("100 - {}", shift(-420))).await;

    assert_eq!(secs, 100 + 25_200);
}
