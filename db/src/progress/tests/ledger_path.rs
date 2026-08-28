//! The forward-progress ledger seen from the *write path* — the wiring
//! `db::progress::ledger`'s own tests take as given. Two surfaces feed it and
//! they carry different payloads: a client that sends a percent (web reader,
//! comic readers, Kobo) is ledgered by the upsert, and a client that sends only
//! a CFI (the iOS reader) is ledgered by the derived-percent attach.

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
    let mark: Option<i64> =
        sqlx::query_scalar("SELECT percent FROM reading_progress_marks WHERE user_id = ?")
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
    let mark: Option<i64> =
        sqlx::query_scalar("SELECT percent FROM reading_progress_marks WHERE user_id = ?")
            .bind(user)
            .fetch_optional(&pool)
            .await
            .unwrap();
    assert_eq!(mark, Some(10));
}
