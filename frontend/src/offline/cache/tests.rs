//! Cache read/write and network-first `read_through` policy tests.

// The state-lock guard is deliberately held across awaits: it serializes
// whole async test bodies against process-global state, and each test owns
// its own thread + runtime, so there is no interleaving to deadlock on.
#![allow(clippy::await_holding_lock)]

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

/// A real connect-refused network error for the offline-fallback arm.
async fn offline_class_error() -> DataError {
    let err = crate::data::http_client()
        .get("http://127.0.0.1:1/nope")
        .send()
        .await
        .expect_err("connect must fail");
    DataError::from(err)
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
async fn read_through_serves_cache_on_offline_error() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    put_json("cache-test:rt-warm", &fx("Cached"));
    let err = offline_class_error().await;
    let out: Result<Fixture, DataError> =
        read_through("cache-test:rt-warm".to_string(), async { Err(err) }).await;
    assert_eq!(out.unwrap(), fx("Cached"));
}

#[tokio::test]
async fn read_through_propagates_offline_error_on_cold_cache() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    let err = offline_class_error().await;
    let out: Result<Fixture, DataError> =
        read_through("cache-test:rt-cold".to_string(), async { Err(err) }).await;
    assert!(matches!(out, Err(DataError::Network(_))));
}

#[tokio::test]
async fn read_through_never_falls_back_on_http_errors() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    put_json("cache-test:rt-http", &fx("Stale"));
    let out: Result<Fixture, DataError> = read_through("cache-test:rt-http".to_string(), async {
        Err(DataError::Http {
            status: 500,
            body: "boom".into(),
        })
    })
    .await;
    // The server answered — its answer wins over stale cache.
    assert!(matches!(out, Err(DataError::Http { status: 500, .. })));
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
    assert_ne!(keys::highlights("u1"), keys::bookmarks("u1"));
}
