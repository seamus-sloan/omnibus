//! The forward-progress ledger seen from the *write path* — the wiring
//! `db::progress::ledger`'s own tests take as given. Three surfaces feed it and
//! they carry different payloads: a client that sends a percent (web reader,
//! comic readers, Kobo) is ledgered by the upsert, and a client that sends only
//! a CFI (the iOS reader) is ledgered by whichever derived attach reaches the
//! row first — `attach_derived_percent` off the request path, or the Kobo
//! sync's `attach_derived_kobo_location`.

use crate::init_db;

use super::*;

const T0: i64 = 1_700_000_000;

/// Percent gained on the one book, by day.
async fn days(pool: &SqlitePool, user: i64) -> Vec<(String, i64)> {
    sqlx::query_as(
        "SELECT day, percent_gained FROM reading_progress_daily
         WHERE user_id = ? ORDER BY day",
    )
    .bind(user)
    .fetch_all(pool)
    .await
    .unwrap()
}

fn epub_update(uuid: &str, cfi: &str, percent: Option<i64>, at: i64) -> ProgressUpdate {
    ProgressUpdate {
        book_uuid: uuid.to_string(),
        format: ProgressFormat::Epub,
        epub_cfi: Some(cfi.to_string()),
        audio_position_seconds: None,
        progress_percent: percent,
        kobo_location: None,
        book_file_id: None,
        client_updated_at: Some(at),
    }
}

#[tokio::test]
async fn upsert_progress_ledgers_the_ground_between_two_percent_writes() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Ledger").await;

    upsert_progress(
        &pool,
        user,
        &epub_update(&uuid, "epubcfi(/6/2)", Some(12), T0),
    )
    .await
    .unwrap();
    upsert_progress(
        &pool,
        user,
        &epub_update(&uuid, "epubcfi(/6/8)", Some(30), T0 + 60),
    )
    .await
    .unwrap();

    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 18)]);
}

#[tokio::test]
async fn upsert_progress_ledgers_nothing_for_a_write_the_clock_guard_rejects() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Ledger").await;

    upsert_progress(
        &pool,
        user,
        &epub_update(&uuid, "epubcfi(/6/8)", Some(30), T0),
    )
    .await
    .unwrap();
    // An older write loses, so the row — and with it the mark — does not move.
    // The ledger follows the surviving position, not the offered one.
    upsert_progress(
        &pool,
        user,
        &epub_update(&uuid, "epubcfi(/6/2)", Some(90), T0 - 60),
    )
    .await
    .unwrap();

    assert!(days(&pool, user).await.is_empty());
}

#[tokio::test]
async fn upsert_progress_ledgers_nothing_for_a_cfi_only_write() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Ledger").await;

    // No percent, no observation. This is the iOS reader's payload; the attach
    // below is what makes it measurable.
    upsert_progress(&pool, user, &epub_update(&uuid, "epubcfi(/6/2)", None, T0))
        .await
        .unwrap();

    assert!(days(&pool, user).await.is_empty());
    let mark: Option<i64> = sqlx::query_scalar(
        "SELECT sitting_max_percent FROM reading_progress_marks WHERE user_id = ?",
    )
    .bind(user)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(mark, None);
}

#[tokio::test]
async fn attach_derived_percent_ledgers_the_gain_for_a_cfi_only_client() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Ledger").await;

    // The iOS shape end to end: each write carries a CFI only and nulls the
    // stored percent, and the derivation refills it off the request path.
    // Differencing the live row would see NULL half the time and lose every
    // gain, which is why the ledger keeps its own mark.
    upsert_progress(&pool, user, &epub_update(&uuid, "epubcfi(/6/2)", None, T0))
        .await
        .unwrap();
    attach_derived_percent(&pool, user, &uuid, 20, T0)
        .await
        .unwrap();

    upsert_progress(
        &pool,
        user,
        &epub_update(&uuid, "epubcfi(/6/8)", None, T0 + 60),
    )
    .await
    .unwrap();
    attach_derived_percent(&pool, user, &uuid, 45, T0 + 60)
        .await
        .unwrap();

    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 25)]);
}

#[tokio::test]
async fn attach_derived_percent_ledgers_nothing_when_the_attach_is_a_no_op() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Ledger").await;

    upsert_progress(
        &pool,
        user,
        &epub_update(&uuid, "epubcfi(/6/2)", Some(10), T0),
    )
    .await
    .unwrap();
    // The row already carries a percent, so the optimistic attach no-ops — and
    // a no-op attach describes no position, so it must not baseline or accrue.
    let attached = attach_derived_percent(&pool, user, &uuid, 90, T0)
        .await
        .unwrap();

    assert!(!attached);
    assert!(days(&pool, user).await.is_empty());
    let mark: Option<i64> = sqlx::query_scalar(
        "SELECT sitting_max_percent FROM reading_progress_marks WHERE user_id = ?",
    )
    .bind(user)
    .fetch_optional(&pool)
    .await
    .unwrap();
    assert_eq!(mark, Some(10));
}

#[tokio::test]
async fn attach_derived_kobo_location_ledgers_the_percent_it_fills() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Ledger").await;

    // A CFI-only write ledgers nothing, so the row sits with a NULL percent and
    // the mark unset.
    upsert_progress(&pool, user, &epub_update(&uuid, "epubcfi(/6/2)", None, T0))
        .await
        .unwrap();
    assert_eq!(days(&pool, user).await, vec![]);

    // The Kobo attach is the writer that fills it here — the request-path
    // derivation would find nothing left to do — so it has to be the one that
    // ledgers, or the gain is lost for good.
    let baselined = attach_derived_kobo_location(&pool, user, &uuid, "{}", Some(20), T0)
        .await
        .unwrap();

    assert!(baselined);
    // First observation baselines, exactly as it does on every other path.
    assert_eq!(days(&pool, user).await, vec![]);

    upsert_progress(
        &pool,
        user,
        &epub_update(&uuid, "epubcfi(/6/8)", Some(45), T0 + 60),
    )
    .await
    .unwrap();

    // 45 - 20: the Kobo-attached percent is the mark the next write differences
    // against, which is the whole point of ledgering it.
    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 25)]);
}

#[tokio::test]
async fn attach_derived_kobo_location_ledgers_the_settled_percent_not_the_offered_one() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, uuid) = seed(&pool, "/lib", "Ledger").await;

    upsert_progress(
        &pool,
        user,
        &epub_update(&uuid, "epubcfi(/6/8)", Some(60), T0),
    )
    .await
    .unwrap();

    // The row already has a percent, so `COALESCE` keeps 60 and `RETURNING`
    // hands back 60 rather than the offered 90. The offer is deliberately
    // *above* the stored value: once the mark became a high-water mark a
    // too-low offer stopped being able to do damage, and only a too-high one
    // discriminates — ledgering it would credit 30% of a book the row never
    // moved to, permanently, since the day buckets only ever accumulate.
    attach_derived_kobo_location(&pool, user, &uuid, "{}", Some(90), T0)
        .await
        .unwrap();
    upsert_progress(
        &pool,
        user,
        &epub_update(&uuid, "epubcfi(/6/12)", Some(70), T0 + 60),
    )
    .await
    .unwrap();

    // 70 - 60. Ledgering the offer would report 30 and then nothing.
    assert_eq!(days(&pool, user).await, vec![("2023-11-14".into(), 10)]);
}
