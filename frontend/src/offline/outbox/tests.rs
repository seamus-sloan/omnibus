//! Outbox coalescing, temp-id lifecycle, and optimistic-apply tests. Tests
//! that touch the process-global store/queue serialize on
//! `sync::test_state_lock` and start by clearing the ops table.

// The state-lock guard is deliberately held across awaits: it serializes
// whole async test bodies against process-global state, and each test owns
// its own thread + runtime, so there is no interleaving to deadlock on.
#![allow(clippy::await_holding_lock)]

use omnibus_shared::{CreateHighlight, HighlightColor, ProgressFormat, ProgressUpdate};

use crate::offline::store;
use crate::offline::sync::test_state_lock;

use super::*;

fn progress_update(uuid: &str) -> ProgressUpdate {
    ProgressUpdate {
        book_uuid: uuid.to_string(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
        audio_position_seconds: None,
    }
}

async fn clear_ops() {
    let st = store::store().expect("test store");
    let ids: Vec<i64> = st.ops_list().await.into_iter().map(|o| o.id).collect();
    st.ops_delete_many(ids).await;
}

#[test]
fn coalesce_keys_group_upserts_and_keep_partial_patches_distinct() {
    let progress = Op::SaveProgress {
        update: progress_update("u1"),
        captured_at: 1,
    };
    assert_eq!(progress.coalesce_key().as_deref(), Some("prog:u1:epub"));

    let set = Op::SetRating {
        update: omnibus_shared::RatingUpdate {
            book_uuid: "u1".into(),
            stars: 4.0,
        },
    };
    let clear = Op::ClearRating { uuid: "u1".into() };
    // Set-then-clear (or vice versa) must collapse to the latest intent.
    assert_eq!(set.coalesce_key(), clear.coalesce_key());

    // Partial shelf patches must never coalesce — a rename followed by a
    // visibility change are two independent steps.
    let shelf_patch = Op::UpdateShelf {
        id: 3,
        req: omnibus_shared::UpdateShelfRequest {
            name: Some("New".into()),
            description: None,
            visibility: None,
            match_mode: None,
            rules: None,
        },
    };
    assert_eq!(shelf_patch.coalesce_key(), None);
}

#[test]
fn remap_id_rewrites_only_matching_references() {
    let mut edit = Op::UpdateHighlightColor {
        id: -3,
        book_uuid: "u1".into(),
        color: HighlightColor::Blue,
    };
    assert!(edit.remap_id(-3, 42));
    assert!(matches!(edit, Op::UpdateHighlightColor { id: 42, .. }));
    assert!(!edit.remap_id(-3, 99));

    let mut add = Op::AddShelfBooks {
        shelf_id: -5,
        book_uuids: vec!["u1".into()],
    };
    assert!(add.remap_id(-5, 7));
    assert!(matches!(add, Op::AddShelfBooks { shelf_id: 7, .. }));

    // Creates never remap (they *produce* the id).
    let mut create = Op::CreateHighlight {
        temp_id: -3,
        input: CreateHighlight {
            book_uuid: "u1".into(),
            epub_cfi_range: "epubcfi(/6/4!/4/2,/1:0,/1:5)".into(),
            color: HighlightColor::Amber,
            text: None,
        },
    };
    assert!(!create.remap_id(-3, 42));
}

#[tokio::test]
async fn queue_create_highlight_synthesizes_temp_record_and_caches_it() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    let input = CreateHighlight {
        book_uuid: "outbox-book-1".into(),
        epub_cfi_range: "epubcfi(/6/4!/4/2,/1:0,/1:5)".into(),
        color: HighlightColor::Green,
        text: Some("a passage".into()),
    };
    let created = queue_create_highlight(&input).await.expect("queued");
    assert!(created.id < 0, "offline creates carry negative temp ids");
    assert_eq!(created.book_uuid, "outbox-book-1");

    // Optimistically visible in the cached list.
    let cached: Vec<omnibus_shared::Highlight> =
        cache::get_json(&cache::keys::highlights("outbox-book-1"))
            .await
            .expect("cached list");
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].id, created.id);

    // And queued for drain.
    let st = store::store().expect("store");
    let ops = st.ops_list().await;
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].kind, "CreateHighlight");
    clear_ops().await;
}

#[tokio::test]
async fn deleting_a_temp_highlight_cancels_its_create_and_edits() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    let input = CreateHighlight {
        book_uuid: "outbox-book-2".into(),
        epub_cfi_range: "epubcfi(/6/4!/4/2,/1:0,/1:5)".into(),
        color: HighlightColor::Amber,
        text: None,
    };
    let created = queue_create_highlight(&input).await.expect("queued");
    assert!(queue_update_highlight_color(created.id, HighlightColor::Rose).await);
    assert!(queue_delete_highlight(created.id).await);

    // Everything referencing the temp id vanished — no server op needed.
    let st = store::store().expect("store");
    assert_eq!(st.ops_count().await, 0);
    let cached: Vec<omnibus_shared::Highlight> =
        cache::get_json(&cache::keys::highlights("outbox-book-2"))
            .await
            .unwrap_or_default();
    assert!(cached.is_empty());
}

#[tokio::test]
async fn progress_coalesces_per_book_and_format_in_the_queue() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    queue_save_progress(&progress_update("book-a"))
        .await
        .expect("q1");
    queue_save_progress(&progress_update("book-b"))
        .await
        .expect("q2");
    let mut newer = progress_update("book-a");
    newer.epub_cfi = Some("epubcfi(/6/12!/4/8/3:7)".into());
    queue_save_progress(&newer).await.expect("q3");

    let st = store::store().expect("store");
    let ops = st.ops_list().await;
    assert_eq!(ops.len(), 2, "same (book, format) coalesces");
    let last: Op = serde_json::from_str(&ops[1].payload).expect("op");
    match last {
        Op::SaveProgress { update, .. } => {
            assert_eq!(update.book_uuid, "book-a");
            assert_eq!(update.epub_cfi.as_deref(), Some("epubcfi(/6/12!/4/8/3:7)"));
        }
        other => panic!("expected SaveProgress, got {other:?}"),
    }
    clear_ops().await;
}

#[tokio::test]
async fn queue_create_shelf_uses_cached_identity_for_owner_fields() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    cache::put_json(
        &cache::keys::me(),
        &omnibus_shared::UserSummary {
            id: 7,
            username: "elena".into(),
            is_admin: false,
            can_upload: true,
            can_edit: true,
            can_download: true,
            kindle_email: None,
        },
    );
    let req = omnibus_shared::CreateShelfRequest {
        kind: omnibus_shared::ShelfKind::Manual,
        name: "Cozy".into(),
        description: None,
        visibility: omnibus_shared::Visibility::Private,
        match_mode: None,
        rules: vec![],
        book_uuids: vec!["u1".into(), "u2".into()],
    };
    let shelf = queue_create_shelf(&req).await.expect("queued");
    assert!(shelf.id < 0);
    assert_eq!(shelf.owner_username, "elena");
    assert_eq!(shelf.book_count, 2);

    let listed: Vec<omnibus_shared::ShelfSummary> = cache::get_json(&cache::keys::shelves())
        .await
        .expect("shelves cached");
    assert!(listed.iter().any(|s| s.id == shelf.id && s.name == "Cozy"));
    clear_ops().await;
}

#[test]
fn fallback_html_escapes_and_paragraphs() {
    let html = fallback_html("Loved <this> & that\n\nSecond\nline");
    assert_eq!(
        html,
        "<p>Loved &lt;this&gt; &amp; that</p><p>Second<br>line</p>"
    );
}
