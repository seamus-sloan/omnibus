//! Write-path concurrency: many simultaneous `upsert_progress` and
//! `record_session` writers on one pool must all succeed rather than trip
//! SQLite's busy handler.

use omnibus_shared::{ProgressUpdate, SessionReport};

use crate::init_db;

use super::super::*;
use super::{seed, seed_user};

// Regression tests for the BEGIN IMMEDIATE stale-snapshot 517 fix (#1862).
const CONCURRENT_ROUNDS: i64 = 5;

const CONCURRENT_WRITERS_PER_ROUND: usize = 5;

#[tokio::test]
async fn upsert_progress_succeeds_for_many_concurrent_writers_on_one_pool() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, shared_uuid) = seed(&pool, "/lib", "Shared Book").await;
    let mut solo_uuids = Vec::new();
    for i in 0..CONCURRENT_WRITERS_PER_ROUND - 1 {
        let (_, uuid) = seed(&pool, "/lib", &format!("Solo Book {i}")).await;
        solo_uuids.push(uuid);
    }

    for round in 0..CONCURRENT_ROUNDS {
        let mut handles = Vec::new();
        for (i, book_uuid) in std::iter::once(shared_uuid.clone())
            .chain(solo_uuids.iter().cloned())
            .enumerate()
        {
            let pool = pool.clone();
            handles.push(tokio::spawn(async move {
                upsert_progress(
                    &pool,
                    user,
                    &ProgressUpdate {
                        book_uuid,
                        format: ProgressFormat::Epub,
                        epub_cfi: Some(format!("epubcfi(/6/4!/4/{i}/1:0)")),
                        audio_position_seconds: None,
                        progress_percent: None,
                        kobo_location: None,
                        book_file_id: None,
                        client_updated_at: Some(1_700_000_000 + round * 10 + i as i64),
                    },
                )
                .await
            }));
        }
        for handle in handles {
            handle
                .await
                .expect("writer task panicked")
                .expect("concurrent upsert_progress must not surface a database error");
        }
    }
}

#[tokio::test]
async fn record_session_succeeds_for_many_concurrent_writers_on_one_pool() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let (_, shared_uuid) = seed(&pool, "/lib", "Shared Book").await;
    let mut solo_uuids = Vec::new();
    for i in 0..CONCURRENT_WRITERS_PER_ROUND - 1 {
        let (_, uuid) = seed(&pool, "/lib", &format!("Solo Session Book {i}")).await;
        solo_uuids.push(uuid);
    }

    for round in 0..CONCURRENT_ROUNDS {
        let mut handles = Vec::new();
        for (i, book_uuid) in std::iter::once(shared_uuid.clone())
            .chain(solo_uuids.iter().cloned())
            .enumerate()
        {
            let pool = pool.clone();
            let started_at = round * 1000 + i as i64;
            handles.push(tokio::spawn(async move {
                record_session(
                    &pool,
                    user,
                    &SessionReport {
                        book_uuid,
                        format: ProgressFormat::Epub,
                        started_at,
                        ended_at: started_at + 60,
                        progress_units: 60,
                        device_id: None,
                        client_id: None,
                    },
                )
                .await
            }));
        }
        for handle in handles {
            let inserted = handle
                .await
                .expect("writer task panicked")
                .expect("concurrent record_session must not surface a database error");
            assert!(inserted, "known uuid must always insert");
        }
    }
}
