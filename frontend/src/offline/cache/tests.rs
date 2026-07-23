//! Cache read/write and cache-first (SWR) `read_through` policy tests.

// The state-lock guard is deliberately held across awaits: it serializes
// whole async test bodies against process-global state, and each test owns
// its own thread + runtime, so there is no interleaving to deadlock on.
#![allow(clippy::await_holding_lock)]

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::data::DataError;
use crate::offline::store;
use crate::offline::sync::test_state_lock;

use super::*;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Fixture {
    title: String,
}

fn fx(title: &str) -> Fixture {
    Fixture {
        title: title.into(),
    }
}

/// A real connect-refused network error for the offline-classification arms.
async fn offline_class_error() -> DataError {
    let err = crate::data::http_client()
        .get("http://127.0.0.1:1/nope")
        .send()
        .await
        .expect_err("connect must fail");
    DataError::from(err)
}

/// Age the cached row so `read_through` treats it as stale.
async fn backdate(key: &str, secs: i64) {
    let key = key.to_string();
    store::store()
        .expect("store")
        .run(move |conn| {
            conn.execute(
                "UPDATE cache SET fetched_at = fetched_at - ?1 WHERE key = ?2",
                rusqlite::params![secs, key],
            )
            .ok();
        })
        .await;
}

/// Poll until the key's background revalidation has finished (or panic).
async fn wait_revalidation_settled(key: &str) {
    for _ in 0..400 {
        let in_flight = revalidating().lock().unwrap().contains(key);
        if !in_flight {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    panic!("revalidation for {key} never settled");
}

#[tokio::test]
async fn put_and_get_json_round_trip() {
    store::init_global_for_tests();
    put_json("cache-test:round", &fx("A Sea of Glass"));
    let got: Option<Fixture> = get_json("cache-test:round").await;
    assert_eq!(got, Some(fx("A Sea of Glass")));
}

#[tokio::test]
async fn get_json_drops_undecodable_payloads() {
    store::init_global_for_tests();
    store::store()
        .expect("store")
        .kv_put("cache-test:garbage", "{not json".to_string());
    let got: Option<Fixture> = get_json("cache-test:garbage").await;
    assert_eq!(got, None);
}

#[tokio::test]
async fn mutate_json_patches_in_place_and_skips_misses() {
    store::init_global_for_tests();
    put_json("cache-test:mutate", &fx("Old"));
    mutate_json::<Fixture, _>("cache-test:mutate", |f| f.title = "New".into()).await;
    let got: Option<Fixture> = get_json("cache-test:mutate").await;
    assert_eq!(got, Some(fx("New")));
    // A miss is a silent no-op.
    mutate_json::<Fixture, _>("cache-test:mutate-missing", |f| f.title = "X".into()).await;
    let got: Option<Fixture> = get_json("cache-test:mutate-missing").await;
    assert_eq!(got, None);
}

// ── cache-first (SWR) policy ────────────────────────────────────────

#[tokio::test]
async fn read_through_serves_fresh_cache_without_touching_network() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    put_json("cache-test:swr-fresh", &fx("Cached"));
    let fetched = Arc::new(AtomicBool::new(false));
    let flag = fetched.clone();
    let out = read_through("cache-test:swr-fresh".to_string(), async move {
        flag.store(true, Ordering::SeqCst);
        Ok(fx("Fresh"))
    })
    .await;
    assert_eq!(out.unwrap(), fx("Cached"));
    assert!(
        !fetched.load(Ordering::SeqCst),
        "a fresh cache hit must not touch the network"
    );
}

#[tokio::test]
async fn read_through_serves_stale_cache_and_revalidates_in_background() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    put_json("cache-test:swr-stale", &fx("Stale"));
    backdate("cache-test:swr-stale", FRESH_WINDOW_SECS + 10).await;

    let mut rx = subscribe();
    rx.borrow_and_update();
    let out = read_through("cache-test:swr-stale".to_string(), async {
        Ok(fx("Revalidated"))
    })
    .await;
    // The stale copy is served immediately…
    assert_eq!(out.unwrap(), fx("Stale"));
    // …and the background revalidation lands the new payload + a bump.
    tokio::time::timeout(std::time::Duration::from_secs(5), rx.changed())
        .await
        .expect("generation bump within 5s")
        .expect("channel alive");
    let got: Option<Fixture> = get_json("cache-test:swr-stale").await;
    assert_eq!(got, Some(fx("Revalidated")));
    crate::offline::sync::note_online();
}

#[tokio::test]
async fn read_through_skips_generation_bump_when_payload_unchanged() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    put_json("cache-test:swr-same", &fx("Same"));
    backdate("cache-test:swr-same", FRESH_WINDOW_SECS + 10).await;

    let rx = subscribe();
    let before = *rx.borrow();
    let fetched = Arc::new(AtomicBool::new(false));
    let flag = fetched.clone();
    let out = read_through("cache-test:swr-same".to_string(), async move {
        flag.store(true, Ordering::SeqCst);
        Ok(fx("Same"))
    })
    .await;
    assert_eq!(out.unwrap(), fx("Same"));
    wait_revalidation_settled("cache-test:swr-same").await;
    assert!(
        fetched.load(Ordering::SeqCst),
        "a stale row must revalidate"
    );
    assert_eq!(
        *rx.borrow(),
        before,
        "identical payloads must not bump the generation"
    );
    crate::offline::sync::note_online();
}

#[tokio::test]
async fn read_through_fails_fast_offline_on_cold_cache() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    crate::offline::sync::note_offline();
    let fetched = Arc::new(AtomicBool::new(false));
    let flag = fetched.clone();
    let out: Result<Fixture, DataError> =
        read_through("cache-test:swr-cold-offline".to_string(), async move {
            flag.store(true, Ordering::SeqCst);
            Ok(fx("Never"))
        })
        .await;
    assert!(matches!(out, Err(DataError::Offline)));
    assert!(
        !fetched.load(Ordering::SeqCst),
        "known-offline cold reads must skip the network entirely"
    );
    crate::offline::sync::note_online();
}

#[tokio::test]
async fn read_through_writes_cache_on_success() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    let out = read_through("cache-test:rt-ok".to_string(), async { Ok(fx("Fresh")) }).await;
    assert_eq!(out.unwrap(), fx("Fresh"));
    let cached: Option<Fixture> = get_json("cache-test:rt-ok").await;
    assert_eq!(cached, Some(fx("Fresh")));
}

#[tokio::test]
async fn read_through_propagates_offline_error_on_cold_cache() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    let err = offline_class_error().await;
    let out: Result<Fixture, DataError> =
        read_through("cache-test:rt-cold".to_string(), async { Err(err) }).await;
    assert!(matches!(out, Err(DataError::Network(_))));
    crate::offline::sync::note_online();
}

#[tokio::test]
async fn read_through_propagates_http_error_on_cold_cache() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    let out: Result<Fixture, DataError> = read_through("cache-test:rt-http".to_string(), async {
        Err(DataError::Http {
            status: 500,
            body: "boom".into(),
        })
    })
    .await;
    // The server answered — its answer wins; no stale fallback.
    assert!(matches!(out, Err(DataError::Http { status: 500, .. })));
}

#[tokio::test]
async fn read_through_skips_revalidation_while_ops_are_pending() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    put_json("cache-test:swr-pending", &fx("Optimistic"));
    backdate("cache-test:swr-pending", FRESH_WINDOW_SECS + 10).await;

    let st = store::store().expect("store");
    st.ops_append("test", "{}".to_string(), None);
    crate::offline::sync::refresh_pending().await;

    let fetched = Arc::new(AtomicBool::new(false));
    let flag = fetched.clone();
    let out = read_through("cache-test:swr-pending".to_string(), async move {
        flag.store(true, Ordering::SeqCst);
        Ok(fx("Server"))
    })
    .await;
    assert_eq!(out.unwrap(), fx("Optimistic"));
    tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    assert!(
        !fetched.load(Ordering::SeqCst),
        "revalidation must wait until the outbox drains"
    );

    let ids: Vec<i64> = st.ops_list().await.into_iter().map(|o| o.id).collect();
    st.ops_delete_many(ids).await;
    crate::offline::sync::refresh_pending().await;
    crate::offline::sync::note_online();
}

// ── network-first escape hatch ──────────────────────────────────────

#[tokio::test]
async fn read_through_network_first_writes_cache_on_success() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    let out =
        read_through_network_first("cache-test:nf-ok".to_string(), async { Ok(fx("Fresh")) }).await;
    assert_eq!(out.unwrap(), fx("Fresh"));
    let cached: Option<Fixture> = get_json("cache-test:nf-ok").await;
    assert_eq!(cached, Some(fx("Fresh")));
}

#[tokio::test]
async fn read_through_network_first_serves_cache_on_offline_error() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    put_json("cache-test:nf-warm", &fx("Cached"));
    let err = offline_class_error().await;
    let out: Result<Fixture, DataError> =
        read_through_network_first("cache-test:nf-warm".to_string(), async { Err(err) }).await;
    assert_eq!(out.unwrap(), fx("Cached"));
    crate::offline::sync::note_online();
}

#[tokio::test]
async fn read_through_network_first_never_falls_back_on_http_errors() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    put_json("cache-test:nf-http", &fx("Stale"));
    let out: Result<Fixture, DataError> =
        read_through_network_first("cache-test:nf-http".to_string(), async {
            Err(DataError::Http {
                status: 500,
                body: "boom".into(),
            })
        })
        .await;
    // The server answered — its answer wins over stale cache.
    assert!(matches!(out, Err(DataError::Http { status: 500, .. })));
}

#[tokio::test]
async fn read_through_network_first_serves_cache_instantly_when_known_offline() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    put_json("cache-test:nf-offline", &fx("Cached"));
    crate::offline::sync::note_offline();
    let fetched = Arc::new(AtomicBool::new(false));
    let flag = fetched.clone();
    let out = read_through_network_first("cache-test:nf-offline".to_string(), async move {
        flag.store(true, Ordering::SeqCst);
        Ok(fx("Never"))
    })
    .await;
    assert_eq!(out.unwrap(), fx("Cached"));
    assert!(
        !fetched.load(Ordering::SeqCst),
        "known-offline reads must skip the doomed connect"
    );
    crate::offline::sync::note_online();
}

#[tokio::test]
async fn read_through_network_first_fails_fast_when_known_offline_and_cold() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    crate::offline::sync::note_offline();
    let out: Result<Fixture, DataError> =
        read_through_network_first("cache-test:nf-cold-offline".to_string(), async {
            Ok(fx("Never"))
        })
        .await;
    assert!(matches!(out, Err(DataError::Offline)));
    crate::offline::sync::note_online();
}

#[test]
fn keys_are_stable_and_distinct_per_entity() {
    assert_eq!(keys::ebook("u1"), "ebook:u1");
    assert_eq!(keys::manifest("u1", Some(9)), "manifest:u1:9");
    assert_eq!(keys::manifest("u1", None), "manifest:u1:-1");
    assert_eq!(keys::progress("u1", "epub"), "progress:u1:epub");
    assert_eq!(
        keys::shelf_page(3, "title", "asc"),
        "shelf_page:3:title:asc"
    );
    assert_eq!(keys::audio_rate(7, "u1"), "audio_rate:7:u1");
    assert_eq!(keys::recent_progress(4), "recent_progress:4");
    assert_ne!(keys::highlights("u1"), keys::bookmarks("u1"));
}
