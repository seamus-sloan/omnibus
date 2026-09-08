//! `pages_detail`: the audio-only versus empty window distinction,
//! measured and unmeasured books counted apart, the ascending per-day
//! chart, the ledger epoch and the pre-ledger caveat, and the DB-failure
//! path.

use super::super::*;
use super::{listen_session, read_percent, seed_book, seed_lib, T0, T0_DAY};
use crate::init_db;
use crate::test_support::seed_user;

#[tokio::test]
async fn pages_detail_separates_an_audio_only_window_from_an_empty_one() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-audio", None, None).await;
    listen_session(&pool, user, "uuid-audio", T0, 600).await;

    let detail = pages_detail(&pool, user, 0, 0).await.unwrap();

    assert_eq!(detail.audio_books, 1);
    assert_eq!(detail.measured_books, 0);
    assert_eq!(detail.unmeasured_books, 0);
    // The one empty state whose honest headline is zero, not an em-dash.
    assert!(detail.audio_only());
}

#[tokio::test]
async fn pages_detail_reports_an_empty_window_as_not_audio_only() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    let detail = pages_detail(&pool, user, 0, 0).await.unwrap();

    assert_eq!(detail.audio_books, 0);
    assert!(!detail.audio_only());
    assert!(detail.daily.is_empty());
}

#[tokio::test]
async fn pages_detail_counts_measured_and_unmeasured_books_apart() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-known", None, Some(120)).await;
    seed_book(&pool, lib, "uuid-unknown", None, None).await;
    read_percent(&pool, user, "uuid-known", T0_DAY, 50).await;
    read_percent(&pool, user, "uuid-unknown", T0_DAY, 50).await;

    let detail = pages_detail(&pool, user, 0, 0).await.unwrap();

    assert_eq!(detail.measured_books, 1);
    // Real reading the total cannot include — a tile that never says so
    // understates itself without admitting it.
    assert_eq!(detail.unmeasured_books, 1);
    assert!(!detail.audio_only());
}

#[tokio::test]
async fn pages_detail_charts_pages_per_day_ascending() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let lib = seed_lib(&pool).await;
    seed_book(&pool, lib, "uuid-a", None, Some(200)).await;
    read_percent(&pool, user, "uuid-a", "2023-11-15", 10).await;
    read_percent(&pool, user, "uuid-a", "2023-11-14", 25).await;

    let detail = pages_detail(&pool, user, 0, 0).await.unwrap();

    let points: Vec<(String, f64)> = detail
        .daily
        .iter()
        .map(|p| (p.label.clone(), p.value))
        .collect();
    assert_eq!(
        points,
        vec![
            ("2023-11-14".to_string(), 50.0),
            ("2023-11-15".to_string(), 20.0),
        ]
    );
}

#[tokio::test]
async fn pages_detail_carries_the_ledger_epoch_so_the_cutover_can_be_stated() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    let detail = pages_detail(&pool, user, 0, 0).await.unwrap();

    // Reading before this day left no position trail to difference; the
    // surfaces state the date rather than letting the tile change meaning
    // without saying so.
    let since = detail.since_day.expect("migration 0083 records the epoch");
    assert_eq!(since.len(), 10, "expected YYYY-MM-DD, got {since}");
}

#[tokio::test]
async fn pages_detail_flags_a_window_that_opens_before_the_ledger_did() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;

    // The epoch is stamped `date('now')` by the migration, so a window opening
    // at the unix epoch certainly predates it and one opening now does not.
    // The range never enters into it — only where the window actually starts.
    assert!(
        pages_detail(&pool, user, 0, 0)
            .await
            .unwrap()
            .window_predates_ledger
    );

    // CAST: `strftime` returns TEXT, which will not decode as an `i64`.
    let now: i64 = sqlx::query_scalar("SELECT CAST(strftime('%s','now') AS INTEGER)")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        !pages_detail(&pool, user, now, 0)
            .await
            .unwrap()
            .window_predates_ledger
    );
}

#[tokio::test]
async fn pages_detail_propagates_sqlx_error_when_the_ledger_table_is_missing() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("DROP TABLE reading_progress_daily")
        .execute(&pool)
        .await
        .unwrap();

    let err = pages_detail(&pool, 1, 0, 0).await.unwrap_err();

    assert!(matches!(err, StatsError::Sqlx(_)));
}
