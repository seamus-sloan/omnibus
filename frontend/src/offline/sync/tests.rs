//! Connectivity-state and error-classification tests. Real `reqwest`
//! errors are produced against live sockets — a refused connect for the
//! offline class, a garbage-body response for the decode class — so the
//! classifier is exercised on the exact error values production sees.

// The state-lock guard is deliberately held across awaits: it serializes
// whole async test bodies against process-global state, and each test owns
// its own thread + runtime, so there is no interleaving to deadlock on.
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::offline::test_support::{connect_refused_error, decode_error};

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
    use axum::routing::{delete, get, patch, post, put};
    let progress_record = |update: ProgressUpdate| ProgressRecord {
        book_uuid: update.book_uuid,
        format: update.format,
        epub_cfi: update.epub_cfi,
        audio_position_seconds: update.audio_position_seconds,
        progress_percent: update.progress_percent,
        kobo_location: update.kobo_location,
        book_file_id: None,
        updated_at: 4242,
        client_updated_at: update.client_updated_at.unwrap_or(4242),
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
            "/api/progress/sessions",
            post(
                |axum::Json(reports): axum::Json<Vec<omnibus_shared::SessionReport>>| async move {
                    axum::Json(serde_json::json!({ "recorded": reports.len() }))
                },
            ),
        )
        .route(
            "/api/audiobooks/{uuid}/playback-rate",
            put(
                |axum::extract::Path(uuid): axum::extract::Path<String>,
                 axum::Json(update): axum::Json<omnibus_shared::AudiobookPlaybackRateUpdate>| async move {
                    axum::Json(omnibus_shared::AudiobookPlaybackRateRecord {
                        book_uuid: uuid,
                        playback_rate: update.playback_rate,
                        updated_at: 333,
                    })
                },
            ),
        )
        .route(
            "/api/ratings/{uuid}",
            delete(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/api/highlights",
            post(
                |axum::Json(input): axum::Json<omnibus_shared::CreateHighlight>| async move {
                    axum::Json(omnibus_shared::Highlight {
                        id: 55,
                        book_uuid: input.book_uuid,
                        epub_cfi_range: Some(input.epub_cfi_range),
                        color: input.color,
                        note: None,
                        text: input.text,
                        client_id: input.client_id,
                        created_at: 222,
                    })
                },
            ),
        )
        .route(
            "/api/highlights/{id}/color",
            patch(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/api/highlights/{id}/note",
            patch(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/api/highlights/{id}",
            delete(|| async { axum::http::StatusCode::NOT_FOUND }),
        )
        .route(
            "/api/bookmarks",
            post(
                |axum::Json(input): axum::Json<omnibus_shared::CreateBookmark>| async move {
                    axum::Json(omnibus_shared::Bookmark {
                        id: 55,
                        book_uuid: input.book_uuid,
                        position: input.position,
                        title: input.title,
                        client_id: input.client_id,
                        created_at: 111,
                    })
                },
            ),
        )
        .route(
            "/api/bookmarks/{id}",
            put(|| async { axum::http::StatusCode::OK }).delete(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/api/journals",
            post(
                |axum::Json(input): axum::Json<omnibus_shared::CreateJournalEntry>| async move {
                    axum::Json(omnibus_shared::JournalEntry {
                        id: 55,
                        book_uuid: input.book_uuid,
                        author_id: 1,
                        author_name: "elena".into(),
                        author_has_avatar: false,
                        body_html: format!("<p>{}</p>", input.body_md),
                        body_md: input.body_md,
                        progress: input.progress,
                        status: input.status,
                        client_id: input.client_id,
                        created_at: 444,
                        updated_at: 444,
                    })
                },
            ),
        )
        .route(
            "/api/journals/{id}",
            patch(
                |axum::extract::Path(id): axum::extract::Path<i64>,
                 axum::Json(input): axum::Json<omnibus_shared::UpdateJournalEntry>| async move {
                    axum::Json(omnibus_shared::JournalEntry {
                        id,
                        book_uuid: "journal-book".into(),
                        author_id: 1,
                        author_name: "elena".into(),
                        author_has_avatar: false,
                        body_html: format!("<p>{}</p>", input.body_md),
                        body_md: input.body_md,
                        progress: input.progress,
                        status: input.status.unwrap_or_default(),
                        client_id: None,
                        created_at: 444,
                        updated_at: 555,
                    })
                },
            )
            .delete(|| async { axum::http::StatusCode::OK }),
        )
        .route(
            "/api/account/kindle-email",
            post(
                |axum::Json(body): axum::Json<serde_json::Value>| async move {
                    // Preserve the 5xx/retry-budget behavior earlier tests assert for this address.
                    if body.get("email").and_then(|v| v.as_str()) == Some("reader@example.com") {
                        axum::http::StatusCode::INTERNAL_SERVER_ERROR
                    } else {
                        axum::http::StatusCode::OK
                    }
                },
            ),
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
                        sync_to_kobo: false,
                    })
                },
            ),
        )
        .route(
            "/api/shelves/{id}",
            patch(
                |axum::extract::Path(id): axum::extract::Path<i64>,
                 axum::Json(req): axum::Json<omnibus_shared::UpdateShelfRequest>| async move {
                    axum::Json(omnibus_shared::Shelf {
                        id,
                        owner_user_id: 1,
                        owner_username: "elena".into(),
                        kind: omnibus_shared::ShelfKind::Manual,
                        name: req.name.unwrap_or_else(|| "Shelf".into()),
                        description: req.description,
                        visibility: req.visibility.unwrap_or(omnibus_shared::Visibility::Private),
                        accent: None,
                        match_mode: req.match_mode,
                        rules: req.rules.unwrap_or_default(),
                        book_count: 0,
                        sync_to_kobo: req.sync_to_kobo.unwrap_or(false),
                    })
                },
            )
            .delete(|| async { axum::http::StatusCode::OK }),
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
        )
        .route(
            "/api/shelves/{id}/books/{uuid}",
            delete(|| async { axum::http::StatusCode::OK }),
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
    for _ in 0..super::MAX_SERVER_ERROR_ATTEMPTS {
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

// ── drain integration: remaining Op variants (#1307 AC4) ───────────────

#[tokio::test]
async fn drain_replays_set_playback_rate_and_caches_the_server_record() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    enqueue_raw(&Op::SetPlaybackRate {
        uuid: "rate-book".into(),
        update: omnibus_shared::AudiobookPlaybackRateUpdate { playback_rate: 1.5 },
    });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(st.ops_count().await, 0, "replayed op must be removed");
    let cached: Option<Option<omnibus_shared::AudiobookPlaybackRateRecord>> =
        cache::get_json(&cache::keys::playback_rate("rate-book")).await;
    let record = cached.flatten().expect("server record cached after drain");
    assert_eq!(record.playback_rate, 1.5);
    note_online();
}

#[tokio::test]
async fn drain_replays_record_sessions_and_clears_the_queue() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    enqueue_raw(&Op::RecordSessions {
        reports: vec![omnibus_shared::SessionReport {
            book_uuid: "session-book".into(),
            format: ProgressFormat::Audio,
            started_at: 1,
            ended_at: 2,
            progress_units: 60,
            device_id: None,
            client_id: Some("dev-1".into()),
        }],
    });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(
        st.ops_count().await,
        0,
        "a recorded session batch must be removed once replayed"
    );
    note_online();
}

#[tokio::test]
async fn drain_replays_clear_rating_and_clears_the_queue() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    enqueue_raw(&Op::ClearRating {
        uuid: "clear-rating-book".into(),
    });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(st.ops_count().await, 0, "a cleared rating must be removed");
    note_online();
}

#[tokio::test]
async fn drain_remaps_temp_bookmark_id_after_create() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    cache::put_json(
        &cache::keys::bookmarks("bm-book"),
        &vec![omnibus_shared::Bookmark {
            id: -90,
            book_uuid: "bm-book".into(),
            position: "epubcfi(/6/4!/4/2/1:0)".into(),
            title: Some("Draft".into()),
            client_id: None,
            created_at: 100,
        }],
    );
    enqueue_raw(&Op::CreateBookmark {
        temp_id: -90,
        input: omnibus_shared::CreateBookmark {
            book_uuid: "bm-book".into(),
            position: "epubcfi(/6/4!/4/2/1:0)".into(),
            title: Some("Draft".into()),
            client_id: None,
        },
    });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(st.ops_count().await, 0);
    let bookmarks: Vec<omnibus_shared::Bookmark> =
        cache::get_json(&cache::keys::bookmarks("bm-book"))
            .await
            .unwrap_or_default();
    assert!(
        bookmarks.iter().any(|b| b.id == 55),
        "cache must hold the server-assigned bookmark id"
    );
    note_online();
}

#[tokio::test]
async fn drain_replays_update_bookmark_and_delete_bookmark() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    enqueue_raw(&Op::UpdateBookmark {
        id: 55,
        book_uuid: "bm-book-2".into(),
        title: Some("Renamed".into()),
    });
    enqueue_raw(&Op::DeleteBookmark {
        id: 56,
        book_uuid: "bm-book-2".into(),
    });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(
        st.ops_count().await,
        0,
        "both a real-id update and delete must replay and clear"
    );
    note_online();
}

#[tokio::test]
async fn drain_remaps_temp_highlight_id_after_create() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    cache::put_json(
        &cache::keys::highlights("hl-book"),
        &vec![omnibus_shared::Highlight {
            id: -91,
            book_uuid: "hl-book".into(),
            epub_cfi_range: Some("epubcfi(/6/4!/4/2/1:0,/1:5)".into()),
            color: omnibus_shared::HighlightColor::Amber,
            note: None,
            text: Some("a passage".into()),
            client_id: None,
            created_at: 100,
        }],
    );
    enqueue_raw(&Op::CreateHighlight {
        temp_id: -91,
        input: omnibus_shared::CreateHighlight {
            book_uuid: "hl-book".into(),
            epub_cfi_range: "epubcfi(/6/4!/4/2/1:0,/1:5)".into(),
            color: omnibus_shared::HighlightColor::Amber,
            text: Some("a passage".into()),
            client_id: None,
        },
    });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(st.ops_count().await, 0);
    let highlights: Vec<omnibus_shared::Highlight> =
        cache::get_json(&cache::keys::highlights("hl-book"))
            .await
            .unwrap_or_default();
    assert!(
        highlights.iter().any(|h| h.id == 55),
        "cache must hold the server-assigned highlight id"
    );
    note_online();
}

#[tokio::test]
async fn drain_replays_update_highlight_color_and_note() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    enqueue_raw(&Op::UpdateHighlightColor {
        id: 55,
        book_uuid: "hl-book-2".into(),
        color: omnibus_shared::HighlightColor::Green,
    });
    enqueue_raw(&Op::UpdateHighlightNote {
        id: 55,
        book_uuid: "hl-book-2".into(),
        note: Some("worth rereading".into()),
    });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(
        st.ops_count().await,
        0,
        "both color and note edits must replay and clear"
    );
    note_online();
}

#[tokio::test]
async fn drain_remaps_temp_journal_id_after_create() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    cache::put_json(
        &cache::keys::journals("journal-book-1"),
        &vec![omnibus_shared::JournalEntry {
            id: -92,
            book_uuid: "journal-book-1".into(),
            author_id: 1,
            author_name: "elena".into(),
            author_has_avatar: false,
            body_md: "draft".into(),
            body_html: "<p>draft</p>".into(),
            progress: Some(10),
            status: omnibus_shared::JournalStatus::Published,
            client_id: None,
            created_at: 100,
            updated_at: 100,
        }],
    );
    enqueue_raw(&Op::CreateJournal {
        temp_id: -92,
        input: omnibus_shared::CreateJournalEntry {
            book_uuid: "journal-book-1".into(),
            body_md: "draft".into(),
            progress: Some(10),
            status: omnibus_shared::JournalStatus::Published,
            client_id: None,
        },
    });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(st.ops_count().await, 0);
    let entries: Vec<omnibus_shared::JournalEntry> =
        cache::get_json(&cache::keys::journals("journal-book-1"))
            .await
            .unwrap_or_default();
    assert!(
        entries.iter().any(|e| e.id == 55),
        "cache must hold the server-assigned journal id"
    );
    note_online();
}

#[tokio::test]
async fn drain_replays_update_journal_and_delete_journal() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    enqueue_raw(&Op::UpdateJournal {
        id: 55,
        book_uuid: "journal-book-2".into(),
        input: omnibus_shared::UpdateJournalEntry {
            body_md: "revised".into(),
            progress: Some(50),
            status: None,
        },
    });
    enqueue_raw(&Op::DeleteJournal {
        id: 56,
        book_uuid: "journal-book-2".into(),
    });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(
        st.ops_count().await,
        0,
        "both a real-id journal edit and delete must replay and clear"
    );
    note_online();
}

#[tokio::test]
async fn drain_replays_set_kindle_email_success_path() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    enqueue_raw(&Op::SetKindleEmail {
        email: Some("new-address@example.com".into()),
    });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(
        st.ops_count().await,
        0,
        "an address distinct from the fixed 5xx one must succeed and clear"
    );
    note_online();
}

#[tokio::test]
async fn drain_replays_update_shelf_and_delete_shelf() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    enqueue_raw(&Op::UpdateShelf {
        id: 55,
        req: omnibus_shared::UpdateShelfRequest {
            name: Some("Renamed Shelf".into()),
            description: None,
            visibility: None,
            match_mode: None,
            rules: None,
            sync_to_kobo: None,
        },
    });
    enqueue_raw(&Op::DeleteShelf { id: 56 });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(
        st.ops_count().await,
        0,
        "both a real-id shelf edit and delete must replay and clear"
    );
    note_online();
}

#[tokio::test]
async fn drain_replays_add_and_remove_shelf_books() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;
    let base = mock_server().await;
    set_server_url(&base);

    enqueue_raw(&Op::AddShelfBooks {
        shelf_id: 55,
        book_uuids: vec!["u1".into(), "u2".into()],
    });
    enqueue_raw(&Op::RemoveShelfBook {
        shelf_id: 55,
        book_uuid: "u1".into(),
    });
    drain().await;

    let st = store::store().expect("store");
    assert_eq!(
        st.ops_count().await,
        0,
        "both a membership add and remove must replay and clear"
    );
    note_online();
}
