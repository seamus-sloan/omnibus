//! Connectivity-state and error-classification tests. Real `reqwest`
//! errors are produced against live sockets — a refused connect for the
//! offline class, a garbage-body response for the decode class — so the
//! classifier is exercised on the exact error values production sees.

// The state-lock guard is deliberately held across awaits: it serializes
// whole async test bodies against process-global state, and each test owns
// its own thread + runtime, so there is no interleaving to deadlock on.
#![allow(clippy::await_holding_lock)]

use super::*;

/// A real connect-refused `DataError::Network` (port 1 is never listening).
async fn connect_refused_error() -> DataError {
    let err = crate::data::http_client()
        .get("http://127.0.0.1:1/nope")
        .send()
        .await
        .expect_err("connect must fail");
    DataError::from(err)
}

/// A real decode-class `DataError::Network`: the server answers 200 with a
/// body that isn't the expected JSON shape.
async fn decode_error() -> DataError {
    use axum::routing::get;
    let app = axum::Router::new().route("/j", get(|| async { "not-json" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let err = crate::data::http_client()
        .get(format!("http://127.0.0.1:{port}/j"))
        .send()
        .await
        .expect("send")
        .json::<Vec<i64>>()
        .await
        .expect_err("decode must fail");
    DataError::from(err)
}

#[tokio::test]
async fn is_offline_error_accepts_connect_failures() {
    let e = connect_refused_error().await;
    assert!(
        is_offline_error(&e),
        "connect-refused must classify offline"
    );
}

#[tokio::test]
async fn is_offline_error_rejects_decode_failures() {
    let e = decode_error().await;
    assert!(
        matches!(e, DataError::Network(_)),
        "mobile decode failures surface as Network"
    );
    assert!(
        !is_offline_error(&e),
        "decode failure means the server WAS reachable"
    );
}

#[test]
fn is_offline_error_rejects_http_and_unauthorized() {
    assert!(!is_offline_error(&DataError::Http {
        status: 500,
        body: String::new()
    }));
    assert!(!is_offline_error(&DataError::Unauthorized));
    assert!(!is_offline_error(&DataError::Other("x".into())));
}

#[test]
fn is_offline_error_accepts_the_fast_fail_offline_variant() {
    assert!(
        is_offline_error(&DataError::Offline),
        "a precheck fast-fail is the same class as a failed connect"
    );
}

#[test]
fn note_offline_and_note_online_flip_the_watch_state() {
    let _guard = test_state_lock().lock().unwrap();
    note_offline();
    assert!(is_offline());
    assert!(!state().online);
    note_online();
    assert!(!is_offline());
    assert!(state().online);
}

#[test]
fn note_dropped_accumulates_only_nonzero_counts() {
    let _guard = test_state_lock().lock().unwrap();
    let before = state().dropped_ops;
    note_dropped(0);
    assert_eq!(state().dropped_ops, before);
    note_dropped(2);
    assert_eq!(state().dropped_ops, before + 2);
}

// ── drain integration (mock server) ─────────────────────────────────

use omnibus_shared::{ProgressFormat, ProgressRecord, ProgressUpdate};

use crate::offline::cache;

async fn clear_ops() {
    let st = store::store().expect("test store");
    let ids: Vec<i64> = st.ops_list().await.into_iter().map(|o| o.id).collect();
    st.ops_delete_many(ids).await;
}

fn enqueue_raw(op: &Op) {
    let st = store::store().expect("test store");
    st.ops_append(
        op.kind(),
        serde_json::to_string(op).expect("payload"),
        op.coalesce_key(),
    );
}

/// Mock server covering the drain-test routes; returns its base URL.
async fn mock_server() -> String {
    use axum::routing::{delete, get, post};
    let progress_record = |update: ProgressUpdate| ProgressRecord {
        book_uuid: update.book_uuid,
        format: update.format,
        epub_cfi: update.epub_cfi,
        audio_position_seconds: update.audio_position_seconds,
        updated_at: 4242,
    };
    let app = axum::Router::new()
        .route(
            "/api/progress/{uuid}",
            get(|| async { axum::Json(None::<ProgressRecord>) }),
        )
        .route(
            "/api/progress",
            post(
                move |axum::Json(update): axum::Json<ProgressUpdate>| async move {
                    axum::Json(progress_record(update))
                },
            ),
        )
        .route(
            "/api/highlights/{id}",
            delete(|| async { axum::http::StatusCode::NOT_FOUND }),
        )
        .route(
            "/api/ratings",
            post(|| async { axum::http::StatusCode::BAD_REQUEST }),
        )
        .route(
            "/api/shelves",
            post(
                |axum::Json(req): axum::Json<omnibus_shared::CreateShelfRequest>| async move {
                    axum::Json(omnibus_shared::Shelf {
                        id: 55,
                        owner_user_id: 1,
                        owner_username: "elena".into(),
                        kind: req.kind,
                        name: req.name,
                        description: req.description,
                        visibility: req.visibility,
                        accent: None,
                        match_mode: req.match_mode,
                        rules: req.rules,
                        book_count: 0,
                    })
                },
            ),
        )
        .route(
            "/api/shelves/{id}/books",
            post(
                |axum::extract::Path(id): axum::extract::Path<i64>| async move {
                    // The remapped id must be the server-assigned one.
                    if id == 55 {
                        axum::http::StatusCode::OK
                    } else {
                        axum::http::StatusCode::NOT_FOUND
                    }
                },
            ),
        );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{port}")
}

fn progress_op(uuid: &str) -> Op {
    Op::SaveProgress {
        update: ProgressUpdate {
            book_uuid: uuid.to_string(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
            audio_position_seconds: None,
        },
        captured_at: 100,
    }
}

#[tokio::test]
async fn background_tick_is_a_noop_while_offline() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    set_server_url("http://127.0.0.1:1");
    note_offline();
    background_tick().await;
    assert!(
        is_offline(),
        "an offline tick must not probe or flip the state"
    );
    note_online();
}

#[tokio::test]
async fn drain_bumps_the_cache_generation_after_resolving_ops() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    let rx = cache::subscribe();
    let before = *rx.borrow();
    enqueue_raw(&progress_op("drain-book-gen"));
    drain().await;

    assert!(
        *rx.borrow() > before,
        "resolving ops must nudge open pages to re-read the cache"
    );
    note_online();
}

#[tokio::test]
async fn drain_replays_progress_and_clears_the_queue() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    enqueue_raw(&progress_op("drain-book-1"));
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(st.ops_count().await, 0, "replayed op must be removed");
    let cached: Option<Option<ProgressRecord>> =
        cache::get_json(&cache::keys::progress("drain-book-1", "epub")).await;
    let record = cached.flatten().expect("server record cached after drain");
    assert_eq!(record.updated_at, 4242);
    note_online();
}

#[tokio::test]
async fn drain_treats_404_on_delete_as_success() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    let dropped_before = state().dropped_ops;
    enqueue_raw(&Op::DeleteHighlight {
        id: 999,
        book_uuid: "gone".into(),
    });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(st.ops_count().await, 0);
    assert_eq!(
        state().dropped_ops,
        dropped_before,
        "a 404'd delete is success, not a rejection"
    );
    note_online();
}

#[tokio::test]
async fn drain_drops_permanently_rejected_ops_and_counts_them() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    let dropped_before = state().dropped_ops;
    enqueue_raw(&Op::SetRating {
        update: omnibus_shared::RatingUpdate {
            book_uuid: "drain-book-2".into(),
            stars: 4.5,
        },
    });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(
        st.ops_count().await,
        0,
        "rejected op must not wedge the queue"
    );
    assert_eq!(state().dropped_ops, dropped_before + 1);
    note_online();
}

#[tokio::test]
async fn drain_halts_and_keeps_ops_when_the_server_is_unreachable() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    set_server_url("http://127.0.0.1:1");

    enqueue_raw(&progress_op("drain-book-3"));
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(
        st.ops_count().await,
        1,
        "op must survive for the next drain"
    );
    assert!(
        is_offline(),
        "an unreachable server flips the state offline"
    );
    clear_ops().await;
    note_online();
}

#[tokio::test]
async fn drain_remaps_temp_shelf_ids_across_later_ops() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    // Simulate the optimistic apply the queue path would have done.
    cache::put_json(
        &cache::keys::shelves(),
        &vec![omnibus_shared::ShelfSummary {
            id: -80,
            owner_user_id: 1,
            owner_username: "elena".into(),
            kind: omnibus_shared::ShelfKind::Manual,
            name: "Drain Shelf".into(),
            visibility: omnibus_shared::Visibility::Private,
            accent: None,
            book_count: 0,
        }],
    );
    enqueue_raw(&Op::CreateShelf {
        temp_id: -80,
        req: omnibus_shared::CreateShelfRequest {
            kind: omnibus_shared::ShelfKind::Manual,
            name: "Drain Shelf".into(),
            description: None,
            visibility: omnibus_shared::Visibility::Private,
            match_mode: None,
            rules: vec![],
            book_uuids: vec![],
        },
    });
    enqueue_raw(&Op::AddShelfBooks {
        shelf_id: -80,
        book_uuids: vec!["u1".into()],
    });
    drain().await;

    // Both ops replayed: the mock 404s any add-books call that didn't get
    // the remapped id 55, which would have left a dropped op behind.
    let st = store::store().expect("store");
    assert_eq!(st.ops_count().await, 0);
    let shelves: Vec<omnibus_shared::ShelfSummary> = cache::get_json(&cache::keys::shelves())
        .await
        .unwrap_or_default();
    assert!(
        shelves
            .iter()
            .any(|s| s.id == 55 && s.name == "Drain Shelf"),
        "cache must hold the server-assigned shelf id"
    );
    note_online();
}
