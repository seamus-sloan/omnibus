//! The drain loop's contract: a no-op while offline, the cache generation
//! bump, a 404 on delete treated as success, permanently rejected ops
//! dropped and counted, an unreachable server halting the drain with the
//! queue intact, temp shelf ids remapped across later ops, one server
//! error not blocking the rest, and the retry budget.

#![allow(clippy::await_holding_lock)]

use super::super::*;
use crate::offline::cache;

use super::{clear_ops, enqueue_raw, mock_server};
use omnibus_shared::{ProgressFormat, ProgressRecord, ProgressUpdate};

fn progress_op(uuid: &str) -> Op {
    Op::SaveProgress {
        update: ProgressUpdate {
            book_uuid: uuid.to_string(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: None,
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
            cover_uuids: Vec::new(),
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

#[tokio::test]
async fn drain_lets_other_ops_through_when_one_gets_a_server_error() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    enqueue_raw(&Op::SetKindleEmail {
        email: Some("reader@example.com".into()),
    });
    enqueue_raw(&progress_op("drain-book-server-error"));
    drain().await;

    let st = store::store().expect("store");
    let remaining = st.ops_list().await;
    assert_eq!(
        remaining.len(),
        1,
        "the 5xx op stays queued but must not block the other op"
    );
    assert_eq!(remaining[0].kind, "SetKindleEmail");
    assert_eq!(
        remaining[0].attempts, 1,
        "a server-error attempt must be recorded"
    );
    let cached: Option<Option<ProgressRecord>> =
        cache::get_json(&cache::keys::progress("drain-book-server-error", "epub")).await;
    assert!(
        cached.flatten().is_some(),
        "the unrelated op behind the stuck one must still drain"
    );
    assert!(
        state().online,
        "a 5xx from one op's target must not flip the device offline"
    );
    clear_ops().await;
    note_online();
}

#[tokio::test]
async fn drain_drops_a_stuck_op_after_exhausting_its_retry_budget() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    let stuck_before = state().stuck_ops;
    enqueue_raw(&Op::SetKindleEmail {
        email: Some("reader@example.com".into()),
    });
    for _ in 0..super::super::MAX_SERVER_ERROR_ATTEMPTS {
        drain().await;
    }

    let st = store::store().expect("store");
    assert_eq!(
        st.ops_count().await,
        0,
        "an op that never stops 5xx'ing must eventually be dropped"
    );
    assert_eq!(
        state().stuck_ops,
        stuck_before + 1,
        "an exhausted retry budget must count as stuck, not silently vanish"
    );
    assert!(
        state().online,
        "exhausting a retry budget is still not a connectivity failure"
    );
    note_online();
}
