//! The ledger's calendar: a quarter-hour gain re-buckets onto the
//! reader's own day for any offset, both ledger generations are read, a
//! frozen row keeps its stored day, the window and day prunes change no
//! answer, and both arms seek their index.

use super::super::*;
use super::{read_percent, read_percent_at, seed_book, seed_lib, set_print_pages, T0, T0_DAY};
use crate::init_db;
use crate::test_support::seed_user;

/// Set up one 400-page book with a gain observed at `at`, and return the user.
async fn seed_evening_read(pool: &SqlitePool, at: i64, gained: i64) -> i64 {
    let user = seed_user(pool, "alice").await;
    let lib = seed_lib(pool).await;
    seed_book(pool, lib, "uuid-a", None, None).await;
    set_print_pages(pool, "uuid-a", 400).await;
    read_percent_at(pool, user, "uuid-a", at, gained).await;
    user
}

#[tokio::test]
async fn pages_read_on_day_counts_an_evening_gain_toward_the_readers_own_day() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // 2023-11-14 21:00 in Los Angeles is already the 15th in UTC. This is the
    // bug the whole change exists for: a reader at UTC-8 turning pages in the
    // evening watched their daily goal reset at 4pm, because the ledger filed
    // the gain under the UTC day.
    let evening_in_la = 1_700_024_400;
    let user = seed_evening_read(&pool, evening_in_la, 25).await;

    let la = pages_read_on_day(&pool, user, "2023-11-14", -480)
        .await
        .unwrap();
    let utc = pages_read_on_day(&pool, user, "2023-11-14", 0)
        .await
        .unwrap();

    assert_eq!(la, 100, "the reader's own evening");
    assert_eq!(utc, 0, "and it is the next day in UTC");
}

#[tokio::test]
async fn pages_read_on_day_rebuckets_one_stored_gain_for_any_zone() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let evening_in_la = 1_700_024_400;
    let user = seed_evening_read(&pool, evening_in_la, 25).await;

    // One row, three calendars, no rewrite — what keying on a quarter-hour
    // buys that a stored day string never could.
    for (offset, day, expected) in [
        (-480, "2023-11-14", 100),
        (0, "2023-11-15", 100),
        (540, "2023-11-15", 100),
    ] {
        let got = pages_read_on_day(&pool, user, day, offset).await.unwrap();
        assert_eq!(got, expected, "offset {offset} on {day}");
    }
}

#[tokio::test]
async fn pages_read_reads_both_ledger_generations() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, None).await;
    set_print_pages(&pool, "uuid-a", 400).await;
    // A row from before the cutover and one after it. Dropping either would
    // silently lose a reader's history at the migration boundary.
    read_percent(&pool, user, "uuid-a", T0_DAY, 10).await;
    read_percent_at(&pool, user, "uuid-a", T0, 15).await;

    assert_eq!(pages_read(&pool, user, 0, 0).await.unwrap(), Some(100));
}

#[tokio::test]
async fn a_frozen_ledger_row_keeps_its_stored_day_whatever_the_offset() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, None).await;
    set_print_pages(&pool, "uuid-a", 400).await;
    read_percent(&pool, user, "uuid-a", T0_DAY, 25).await;

    // Pre-0093 rows kept no instant, so there is nothing to re-bucket them
    // from. Passing them through unshifted is the honest answer; inventing an
    // instant would re-date history that was never re-datable.
    for offset in [-480, 0, 540] {
        let got = pages_read_on_day(&pool, user, T0_DAY, offset)
            .await
            .unwrap();
        assert_eq!(got, 100, "offset {offset} moved a frozen row");
    }
}

/// Offsets spanning the resolver's whole clamp, including the quarter-hour
/// zones an hourly grid could not place (Newfoundland -03:30, Nepal +05:45).
const PRUNE_OFFSETS: [i64; 6] = [-720, -480, -210, 0, 345, 840];

/// UTC midnight of [`T0_DAY`]; every instant below is placed against it.
const T0_MIDNIGHT: i64 = 1_699_920_000;

/// The `YYYY-MM-DD` SQLite puts a unix second in — the same expression the
/// frozen ledger rows were written with.
async fn utc_day(pool: &SqlitePool, at: i64) -> String {
    sqlx::query_scalar("SELECT date(?, 'unixepoch')")
        .bind(at)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// [`ledger_days`] with neither arm bounded: the shape every aggregate here had
/// before the prune, and so the oracle for it. Built from the same function, so
/// the two can never drift into being different unions.
fn unpruned_ledger_days(offset_minutes: i64) -> String {
    ledger_days(
        offset_minutes,
        &LedgerPrune {
            slots: "1 = 1".to_string(),
            daily: "1 = 1".to_string(),
        },
    )
}

/// [`pages_read`] with the prune removed; same three binds.
async fn unpruned_pages_read(
    pool: &SqlitePool,
    user_id: i64,
    start: i64,
    offset_minutes: i64,
) -> Option<i64> {
    let sql = format!(
        "SELECT CAST(ROUND(SUM(CAST(g.percent_gained AS REAL) * p.pages) / 100.0) AS INTEGER)
         FROM ({}) g
         JOIN ({}) p ON p.uuid = g.uuid
         WHERE p.pages IS NOT NULL AND g.day >= {}",
        unpruned_ledger_days(offset_minutes),
        book_pages_source(),
        calendar::local_day("?3", offset_minutes)
    );
    sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(start)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// [`pages_read_on_day`] with the prune removed; same three binds.
async fn unpruned_pages_read_on_day(
    pool: &SqlitePool,
    user_id: i64,
    day: &str,
    offset_minutes: i64,
) -> i64 {
    let sql = format!(
        "SELECT COALESCE(
                    CAST(ROUND(SUM(CAST(g.percent_gained AS REAL) * p.pages) / 100.0) AS INTEGER),
                    0)
         FROM ({}) g
         JOIN ({}) p ON p.uuid = g.uuid
         WHERE g.day = ?3 AND p.pages IS NOT NULL",
        unpruned_ledger_days(offset_minutes),
        book_pages_source()
    );
    sqlx::query_scalar(&sql)
        .bind(user_id)
        .bind(user_id)
        .bind(day)
        .fetch_one(pool)
        .await
        .unwrap()
}

/// A reader with gains straddling every edge a prune can land on: the quarter
/// hours either side of each UTC midnight across a week, plus midday, in both
/// ledger generations.
async fn seed_a_week_of_gains(pool: &SqlitePool) -> i64 {
    let user = seed_user(pool, "alice").await;
    let lib = seed_lib(pool).await;
    seed_book(pool, lib, "uuid-a", None, Some(400)).await;
    for day in -3..=3_i64 {
        let midnight = T0_MIDNIGHT + day * 86_400;
        for at in [midnight - 900, midnight, midnight + 900, midnight + 43_200] {
            read_percent_at(pool, user, "uuid-a", at, 1).await;
        }
        let stored = utc_day(pool, midnight).await;
        read_percent(pool, user, "uuid-a", &stored, 1).await;
    }
    user
}

#[tokio::test]
async fn the_window_prune_changes_no_pages_read_answer_at_any_offset() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_a_week_of_gains(&pool).await;

    // Not a vacuous comparison: the widest window really does resolve pages.
    assert!(pages_read(&pool, user, 0, 0).await.unwrap().is_some());

    // The bound is an implication of the outer day filter, never a filter in
    // its own right, so it must move no figure — least of all at an edge, and
    // least of all where a quarter-hour zone puts that edge mid-slot.
    for offset in PRUNE_OFFSETS {
        for start in [
            0,
            T0_MIDNIGHT - 3 * 86_400,
            T0_MIDNIGHT - 1,
            T0_MIDNIGHT,
            T0_MIDNIGHT + 1,
            T0_MIDNIGHT + 3 * 86_400,
        ] {
            assert_eq!(
                pages_read(&pool, user, start, offset).await.unwrap(),
                unpruned_pages_read(&pool, user, start, offset).await,
                "offset {offset}, start {start}"
            );
        }
    }
}

#[tokio::test]
async fn the_day_prune_changes_no_daily_goal_answer_at_any_offset() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_a_week_of_gains(&pool).await;

    assert!(pages_read_on_day(&pool, user, T0_DAY, 345).await.unwrap() > 0);

    // Both ends this time, and a day either side of the seeded week so the
    // bounds are exercised from outside as well as within.
    for offset in PRUNE_OFFSETS {
        for day in -4..=4_i64 {
            let day = utc_day(&pool, T0_MIDNIGHT + day * 86_400).await;
            assert_eq!(
                pages_read_on_day(&pool, user, &day, offset).await.unwrap(),
                unpruned_pages_read_on_day(&pool, user, &day, offset).await,
                "offset {offset} on {day}"
            );
        }
    }
}

/// UTC instant of the local midnight opening 2023-11-15 in Nepal (UTC+05:45) —
/// 18:15 the previous afternoon in UTC, so gains either side of it share a UTC
/// day and land on different days on the reader's own calendar.
const NEPAL_MIDNIGHT: i64 = 1_699_985_700;

#[tokio::test]
async fn pages_read_bounded_slices_on_the_readers_day_at_a_quarter_hour_offset() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(400)).await;
    read_percent_at(&pool, user, "uuid-a", NEPAL_MIDNIGHT - 900, 10).await;
    read_percent_at(&pool, user, "uuid-a", NEPAL_MIDNIGHT, 10).await;

    let start = NEPAL_MIDNIGHT - 86_400;
    // `end` is this query's `?4`, the one placeholder outside the window's
    // three. A mis-numbered one would compare the slice against a user id and
    // silently empty it.
    let one_day = pages_read_bounded(&pool, user, start, NEPAL_MIDNIGHT, 345)
        .await
        .unwrap();
    let two_days = pages_read_bounded(&pool, user, start, NEPAL_MIDNIGHT + 86_400, 345)
        .await
        .unwrap();

    assert_eq!(one_day, 40, "their 14th alone");
    assert_eq!(two_days, 80, "and their 15th with it");
}

#[tokio::test]
async fn both_ledger_arms_seek_their_index_rather_than_scan_a_whole_history() {
    let pool = init_db("sqlite::memory:").await.unwrap();

    // What the prune is *for*: with the day predicate applied only outside the
    // union, the slots arm matched `user_id` alone and re-read every row a
    // reader had ever written, once per aggregate, on every stats load. The
    // answers are identical either way, so the plan is the only place it shows.
    let windowed = ledger_plan(&pool, &ledger_in_window(0), "1700000000").await;
    assert!(
        windowed.contains("idx_reading_progress_slots_user_slot") && windowed.contains("slot>"),
        "the windowed slots arm should seek on slot, got plan:\n{windowed}"
    );
    assert!(
        windowed.contains("idx_reading_progress_daily_user_day") && windowed.contains("day>"),
        "the windowed frozen arm should seek on day, got plan:\n{windowed}"
    );

    let by_day = ledger_plan(&pool, &ledger_days(0, &day_prune("?3")), T0_DAY).await;
    assert!(
        by_day.contains("slot>") && by_day.contains("slot<"),
        "a single day should bound the slots arm at both ends, got plan:\n{by_day}"
    );
    assert!(
        by_day.contains("day=?"),
        "a single day should seek the frozen arm exactly, got plan:\n{by_day}"
    );
}

/// The `EXPLAIN QUERY PLAN` of one ledger query, newline-joined. A plan does not
/// depend on the values bound, so the two user ids are arbitrary and `bound` is
/// whichever of a window start or a day the query takes as `?3`.
async fn ledger_plan(pool: &SqlitePool, sql: &str, bound: &str) -> String {
    let explain = format!("EXPLAIN QUERY PLAN {sql}");
    sqlx::query(&explain)
        .bind(1_i64)
        .bind(1_i64)
        .bind(bound)
        .fetch_all(pool)
        .await
        .unwrap()
        .iter()
        .map(|r| r.get::<String, _>("detail"))
        .collect::<Vec<_>>()
        .join("\n")
}
