//! Unit tests for the chart builder, split by sub-topic into the sibling
//! modules below; the fixture, spec and seeding helpers they share live
//! here (the session and completion seeds come from `stats::tests`).

mod axes;
mod breakdown;
mod bucketing;
mod measures;

use omnibus_shared::{ChartBreakdown, ChartSpec, StatsRange};

use super::*;
use crate::init_db;
use crate::stats::tests::seed_user;
use crate::test_support::seed_minimal_books;

/// A pool with `count` books and one user, ready for session/completion seeds.
async fn fixture(count: i64) -> (sqlx::SqlitePool, i64) {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_minimal_books(&pool, count).await;
    let user = seed_user(&pool, "reader").await;
    (pool, user)
}

/// Give a book a real `page_count` so the length ladder's first rung resolves.
async fn set_pages(pool: &sqlx::SqlitePool, uuid: &str, pages: i64) {
    sqlx::query("UPDATE books SET page_count = ? WHERE uuid = ?")
        .bind(pages)
        .bind(uuid)
        .execute(pool)
        .await
        .unwrap();
}

/// A forward-progress ledger row on a given UTC day.
async fn ledger_day(pool: &sqlx::SqlitePool, user: i64, uuid: &str, day: &str, percent: i64) {
    sqlx::query(
        "INSERT INTO reading_progress_daily (user_id, book_uuid, format, day, percent_gained)
         VALUES (?, ?, 'epub', ?, ?)",
    )
    .bind(user)
    .bind(uuid)
    .bind(day)
    .bind(percent)
    .execute(pool)
    .await
    .unwrap();
}

fn spec(measures: Vec<ChartMeasure>, bucket: ChartBucket, range: StatsRange) -> ChartSpec {
    ChartSpec {
        measures,
        bucket,
        range,
        breakdown: ChartBreakdown::None,
    }
}

/// The value a named series carries in the bucket at `idx`.
fn at(result: &ChartResult, series: usize, idx: usize) -> Option<f64> {
    result.series[series].values[idx]
}
