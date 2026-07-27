//! Outbox coalescing, temp-id lifecycle, and optimistic-apply tests. Tests
//! that touch the process-global store/queue serialize on
//! `sync::test_state_lock` and start by clearing the ops table.

// The state-lock guard is deliberately held across awaits: it serializes
// whole async test bodies against process-global state, and each test owns
// its own thread + runtime, so there is no interleaving to deadlock on.
#![allow(clippy::await_holding_lock)]

use omnibus_shared::{
    CreateBookmark, CreateHighlight, CreateJournalEntry, HighlightColor, ProgressFormat,
    ProgressUpdate,
};

use crate::offline::store;
use crate::offline::sync::test_state_lock;

use super::*;

fn progress_update(uuid: &str) -> ProgressUpdate {
    ProgressUpdate {
        book_uuid: uuid.to_string(),
        format: ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
        audio_position_seconds: None,
        progress_percent: None,
        kobo_location: None,
        book_file_id: None,
        client_updated_at: None,
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
            sync_to_kobo: None,
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
            client_id: None,
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
        client_id: None,
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
        client_id: None,
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

fn create_bookmark(uuid: &str) -> CreateBookmark {
    CreateBookmark {
        client_id: None,
        book_uuid: uuid.to_string(),
        position: "epubcfi(/6/4!/4/2/1:0)".into(),
        title: Some("Chapter 1".into()),
    }
}

fn create_journal(uuid: &str) -> CreateJournalEntry {
    CreateJournalEntry {
        client_id: None,
        book_uuid: uuid.to_string(),
        body_md: "Loved this chapter".into(),
        progress: Some(42),
        status: omnibus_shared::JournalStatus::Published,
    }
}

fn test_shelf(id: i64, name: &str, book_count: i64) -> omnibus_shared::Shelf {
    omnibus_shared::Shelf {
        id,
        owner_user_id: 1,
        owner_username: "elena".into(),
        kind: omnibus_shared::ShelfKind::Manual,
        name: name.to_string(),
        description: None,
        visibility: omnibus_shared::Visibility::Private,
        accent: None,
        match_mode: None,
        rules: vec![],
        book_count,
        sync_to_kobo: false,
    }
}

fn test_shelf_summary(id: i64, name: &str, book_count: i64) -> omnibus_shared::ShelfSummary {
    omnibus_shared::ShelfSummary {
        id,
        owner_user_id: 1,
        owner_username: "elena".into(),
        kind: omnibus_shared::ShelfKind::Manual,
        name: name.to_string(),
        visibility: omnibus_shared::Visibility::Private,
        accent: None,
        book_count,
        cover_uuids: Vec::new(),
    }
}

// ── bookmarks ───────────────────────────────────────────────────────

#[tokio::test]
async fn queue_create_bookmark_synthesizes_temp_record_and_caches_it() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    let created = queue_create_bookmark(&create_bookmark("outbox-book-3"))
        .await
        .expect("queued");
    assert!(created.id < 0, "offline creates carry negative temp ids");
    assert_eq!(created.book_uuid, "outbox-book-3");

    let cached: Vec<omnibus_shared::Bookmark> =
        cache::get_json(&cache::keys::bookmarks("outbox-book-3"))
            .await
            .expect("cached list");
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].id, created.id);

    let st = store::store().expect("store");
    let ops = st.ops_list().await;
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].kind, "CreateBookmark");
    clear_ops().await;
}

#[tokio::test]
async fn deleting_a_temp_bookmark_cancels_its_create_and_edits() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    let created = queue_create_bookmark(&create_bookmark("outbox-book-4"))
        .await
        .expect("queued");
    assert!(queue_update_bookmark(created.id, Some("Renamed".into())).await);

    // The rename actually patched the cached record before the cancel wipes it.
    let renamed: Vec<omnibus_shared::Bookmark> =
        cache::get_json(&cache::keys::bookmarks("outbox-book-4"))
            .await
            .expect("cached list");
    assert_eq!(renamed[0].title.as_deref(), Some("Renamed"));

    assert!(queue_delete_bookmark(created.id).await);

    // Everything referencing the temp id vanished — no server op needed.
    let st = store::store().expect("store");
    assert_eq!(st.ops_count().await, 0);
    let cached: Vec<omnibus_shared::Bookmark> =
        cache::get_json(&cache::keys::bookmarks("outbox-book-4"))
            .await
            .unwrap_or_default();
    assert!(cached.is_empty());
}

#[tokio::test]
async fn bookmark_remapped_replaces_the_temp_record_with_the_server_copy() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    let temp = omnibus_shared::Bookmark {
        id: -11,
        book_uuid: "outbox-book-5".into(),
        position: "epubcfi(/6/4!/4/2/1:0)".into(),
        title: None,
        created_at: 1,
        client_id: None,
    };
    apply::bookmark_created(&temp).await;

    let real = omnibus_shared::Bookmark {
        id: 501,
        book_uuid: "outbox-book-5".into(),
        position: "epubcfi(/6/4!/4/2/1:0)".into(),
        title: Some("Server title".into()),
        created_at: 2,
        client_id: None,
    };
    apply::bookmark_remapped(-11, &real).await;

    let cached: Vec<omnibus_shared::Bookmark> =
        cache::get_json(&cache::keys::bookmarks("outbox-book-5"))
            .await
            .expect("cached list");
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].id, 501);
    assert_eq!(cached[0].title.as_deref(), Some("Server title"));
}

// ── highlights ──────────────────────────────────────────────────────

#[tokio::test]
async fn highlight_remapped_replaces_the_temp_record_with_the_server_copy() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    let temp = omnibus_shared::Highlight {
        id: -13,
        book_uuid: "outbox-book-9".into(),
        epub_cfi_range: "epubcfi(/6/4!/4/2,/1:0,/1:5)".into(),
        color: HighlightColor::Blue,
        note: None,
        text: None,
        created_at: 1,
        client_id: None,
    };
    apply::highlight_created(&temp).await;

    let real = omnibus_shared::Highlight {
        id: 901,
        ..temp.clone()
    };
    apply::highlight_remapped(-13, &real).await;

    let cached: Vec<omnibus_shared::Highlight> =
        cache::get_json(&cache::keys::highlights("outbox-book-9"))
            .await
            .expect("cached list");
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].id, 901);
}

// ── shelves ─────────────────────────────────────────────────────────

#[tokio::test]
async fn shelf_update_patches_the_cached_detail_and_list_entry() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    cache::put_json(
        &cache::keys::shelves(),
        &vec![test_shelf_summary(20, "Old", 0)],
    );
    cache::put_json(&cache::keys::shelf(20), &test_shelf(20, "Old", 0));

    let req = omnibus_shared::UpdateShelfRequest {
        name: Some("New".into()),
        description: Some("A cozy pile".into()),
        visibility: Some(omnibus_shared::Visibility::Public),
        match_mode: None,
        rules: None,
        sync_to_kobo: None,
    };
    let patched = queue_update_shelf(20, &req).await.expect("queued");
    assert_eq!(patched.name, "New");
    assert_eq!(patched.description.as_deref(), Some("A cozy pile"));
    assert_eq!(patched.visibility, omnibus_shared::Visibility::Public);

    let detail: omnibus_shared::Shelf = cache::get_json(&cache::keys::shelf(20))
        .await
        .expect("cached detail");
    assert_eq!(detail.name, "New");

    let list: Vec<omnibus_shared::ShelfSummary> = cache::get_json(&cache::keys::shelves())
        .await
        .expect("cached list");
    assert_eq!(list[0].name, "New");
    assert_eq!(list[0].visibility, omnibus_shared::Visibility::Public);
    clear_ops().await;
}

#[tokio::test]
async fn shelf_delete_removes_the_list_detail_and_page_rows() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    cache::put_json(
        &cache::keys::shelves(),
        &vec![test_shelf_summary(30, "Gone Soon", 1)],
    );
    cache::put_json(&cache::keys::shelf(30), &test_shelf(30, "Gone Soon", 1));
    cache::put_json(
        &cache::keys::shelf_page(30, "title", "asc"),
        &omnibus_shared::ShelfPage { books: vec![] },
    );

    assert!(queue_delete_shelf(30).await);

    let list: Vec<omnibus_shared::ShelfSummary> = cache::get_json(&cache::keys::shelves())
        .await
        .expect("cached list");
    assert!(list.is_empty());
    let detail: Option<omnibus_shared::Shelf> = cache::get_json(&cache::keys::shelf(30)).await;
    assert!(detail.is_none());
    let st = store::store().expect("store");
    assert!(st.kv_prefix("shelf_page:30:").await.is_empty());
    clear_ops().await;
}

#[tokio::test]
async fn shelf_books_added_appends_matching_replica_books_and_bumps_counts() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    cache::put_json(
        &cache::keys::shelves(),
        &vec![test_shelf_summary(40, "Reading", 0)],
    );
    cache::put_json(&cache::keys::shelf(40), &test_shelf(40, "Reading", 0));
    cache::put_json(
        &cache::keys::ebooks_all(),
        &vec![omnibus_shared::EbookMetadata {
            id: 1,
            unique_identifier: Some("uuid-a".into()),
            ..Default::default()
        }],
    );
    cache::put_json(
        &cache::keys::shelf_page(40, "title", "asc"),
        &omnibus_shared::ShelfPage { books: vec![] },
    );

    assert!(queue_add_shelf_books(40, &["uuid-a".to_string()]).await);

    let list: Vec<omnibus_shared::ShelfSummary> = cache::get_json(&cache::keys::shelves())
        .await
        .expect("cached list");
    assert_eq!(list[0].book_count, 1);
    let detail: omnibus_shared::Shelf = cache::get_json(&cache::keys::shelf(40))
        .await
        .expect("cached detail");
    assert_eq!(detail.book_count, 1);
    let page: omnibus_shared::ShelfPage =
        cache::get_json(&cache::keys::shelf_page(40, "title", "asc"))
            .await
            .expect("cached page");
    assert_eq!(page.books.len(), 1);
    assert_eq!(page.books[0].unique_identifier.as_deref(), Some("uuid-a"));
    clear_ops().await;
}

#[tokio::test]
async fn shelf_book_removed_drops_the_book_from_cached_pages_and_decrements_count() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    cache::put_json(
        &cache::keys::shelves(),
        &vec![test_shelf_summary(50, "Reading", 1)],
    );
    cache::put_json(&cache::keys::shelf(50), &test_shelf(50, "Reading", 1));
    cache::put_json(
        &cache::keys::shelf_page(50, "title", "asc"),
        &omnibus_shared::ShelfPage {
            books: vec![omnibus_shared::EbookMetadata {
                id: 1,
                unique_identifier: Some("uuid-a".into()),
                ..Default::default()
            }],
        },
    );

    assert!(queue_remove_shelf_book(50, "uuid-a").await);

    let list: Vec<omnibus_shared::ShelfSummary> = cache::get_json(&cache::keys::shelves())
        .await
        .expect("cached list");
    assert_eq!(list[0].book_count, 0);
    let detail: omnibus_shared::Shelf = cache::get_json(&cache::keys::shelf(50))
        .await
        .expect("cached detail");
    assert_eq!(detail.book_count, 0);
    let page: omnibus_shared::ShelfPage =
        cache::get_json(&cache::keys::shelf_page(50, "title", "asc"))
            .await
            .expect("cached page");
    assert!(page.books.is_empty());
    clear_ops().await;
}

#[tokio::test]
async fn shelf_remapped_moves_the_detail_and_page_rows_to_the_real_id() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    cache::put_json(
        &cache::keys::shelves(),
        &vec![test_shelf_summary(-9, "Cozy", 1)],
    );
    cache::put_json(&cache::keys::shelf(-9), &test_shelf(-9, "Cozy", 1));
    cache::put_json(
        &cache::keys::shelf_page(-9, "title", "asc"),
        &omnibus_shared::ShelfPage { books: vec![] },
    );

    let real = test_shelf(600, "Cozy", 1);
    apply::shelf_remapped(-9, &real).await;

    let list: Vec<omnibus_shared::ShelfSummary> = cache::get_json(&cache::keys::shelves())
        .await
        .expect("cached list");
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, 600);

    let old_detail: Option<omnibus_shared::Shelf> = cache::get_json(&cache::keys::shelf(-9)).await;
    assert!(old_detail.is_none());
    let new_detail: omnibus_shared::Shelf = cache::get_json(&cache::keys::shelf(600))
        .await
        .expect("cached detail");
    assert_eq!(new_detail.id, 600);

    let st = store::store().expect("store");
    assert!(st.kv_prefix("shelf_page:-9:").await.is_empty());
    let renamed = st.kv_prefix("shelf_page:600:").await;
    assert_eq!(renamed.len(), 1);
    clear_ops().await;
}

// ── journals ────────────────────────────────────────────────────────

#[tokio::test]
async fn queue_create_journal_synthesizes_temp_record_and_caches_it() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    cache::put_json(
        &cache::keys::me(),
        &omnibus_shared::UserSummary {
            id: 8,
            username: "marcus".into(),
            is_admin: false,
            can_upload: true,
            can_edit: true,
            can_download: true,
            kindle_email: None,
        },
    );

    let created = queue_create_journal(&create_journal("outbox-book-6"))
        .await
        .expect("queued");
    assert!(created.id < 0, "offline creates carry negative temp ids");
    assert_eq!(created.author_name, "marcus");
    assert_eq!(created.body_html, "<p>Loved this chapter</p>");

    let cached: Vec<omnibus_shared::JournalEntry> =
        cache::get_json(&cache::keys::journals("outbox-book-6"))
            .await
            .expect("cached list");
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].id, created.id);

    let st = store::store().expect("store");
    let ops = st.ops_list().await;
    assert_eq!(ops.len(), 1);
    assert_eq!(ops[0].kind, "CreateJournal");
    clear_ops().await;
}

#[tokio::test]
async fn deleting_a_temp_journal_entry_cancels_its_create_and_edits() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    let created = queue_create_journal(&create_journal("outbox-book-7"))
        .await
        .expect("queued");

    let update = omnibus_shared::UpdateJournalEntry {
        body_md: "Edited thoughts".into(),
        progress: Some(55),
        status: Some(omnibus_shared::JournalStatus::Draft),
    };
    let patched = queue_update_journal(created.id, &update)
        .await
        .expect("patched");
    assert_eq!(patched.body_md, "Edited thoughts");
    assert_eq!(patched.progress, Some(55));
    assert_eq!(patched.status, omnibus_shared::JournalStatus::Draft);

    assert!(queue_delete_journal(created.id).await);

    // Everything referencing the temp id vanished — no server op needed.
    let st = store::store().expect("store");
    assert_eq!(st.ops_count().await, 0);
    let cached: Vec<omnibus_shared::JournalEntry> =
        cache::get_json(&cache::keys::journals("outbox-book-7"))
            .await
            .unwrap_or_default();
    assert!(cached.is_empty());
}

#[tokio::test]
async fn journal_remapped_replaces_the_temp_record_with_the_server_copy() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    let temp = omnibus_shared::JournalEntry {
        id: -21,
        book_uuid: "outbox-book-8".into(),
        author_id: 8,
        author_name: "marcus".into(),
        body_md: "draft".into(),
        body_html: "<p>draft</p>".into(),
        progress: None,
        status: omnibus_shared::JournalStatus::Published,
        created_at: 1,
        updated_at: 1,
        client_id: None,
    };
    apply::journal_created(&temp).await;

    let real = omnibus_shared::JournalEntry {
        id: 801,
        ..temp.clone()
    };
    apply::journal_remapped(-21, &real).await;

    let cached: Vec<omnibus_shared::JournalEntry> =
        cache::get_json(&cache::keys::journals("outbox-book-8"))
            .await
            .expect("cached list");
    assert_eq!(cached.len(), 1);
    assert_eq!(cached[0].id, 801);
}

// ── account ─────────────────────────────────────────────────────────

#[tokio::test]
async fn kindle_email_changed_patches_the_cached_account_summary() {
    store::init_global_for_tests();
    let _guard = test_state_lock().lock().unwrap();
    clear_ops().await;

    cache::put_json(
        &cache::keys::me(),
        &omnibus_shared::UserSummary {
            id: 9,
            username: "reader".into(),
            is_admin: false,
            can_upload: true,
            can_edit: true,
            can_download: true,
            kindle_email: None,
        },
    );

    assert!(queue_set_kindle_email(&Some("reader@kindle.com".into())).await);

    let me: omnibus_shared::UserSummary = cache::get_json(&cache::keys::me())
        .await
        .expect("cached me");
    assert_eq!(me.kindle_email.as_deref(), Some("reader@kindle.com"));
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
