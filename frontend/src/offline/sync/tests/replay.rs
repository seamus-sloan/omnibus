//! One replay per queued `Op` kind: playback rate (caching the server
//! record), sessions, rating, the bookmark / highlight / journal
//! create-then-update-then-delete chains with their temp-id remaps, the
//! Kindle email, and shelf updates and membership.

#![allow(clippy::await_holding_lock)]

use super::super::*;
use crate::offline::cache;

use super::{clear_ops, enqueue_raw, mock_server};
use omnibus_shared::ProgressFormat;

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
            utc_offset_minutes: None,
            time_zone: None,
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
