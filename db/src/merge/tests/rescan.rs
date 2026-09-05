//! What a later scan sees after a merge: a chained merge repoints the
//! earlier `merged_uuids` rows to the final target, and the reindex diff
//! classifies the merged source file — and both files of a two-epub book —
//! as Unchanged.

use super::super::*;
use super::book_id_by_uuid;
use crate::books::{list_indexed_rows_for_formats, list_merged_rows_for_formats};
use crate::indexer::diff_library;
use crate::pool::init_db;
use crate::test_support::{
    count_rows as count, indexed_audiobook, seed_synced_audiobook as seed_audiobook,
    seed_synced_ebook as seed_ebook,
};

#[tokio::test]
async fn chained_merge_repoints_earlier_merged_uuids_to_final_target() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let a = seed_audiobook(&pool, "X/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let b = seed_ebook(&pool, "Y/Dracula.epub", "Dracula", "Bram Stoker").await;
    // Auto-attach is title-exact, so a and b stayed separate. A -> B:
    merge_books(&pool, &a, &b, None).await.unwrap();
    // Then B -> C (a PDF-only book, so no format collision).
    let c = seed_ebook(
        &pool,
        "Z/Dracula Deluxe.pdf",
        "Dracula Deluxe",
        "Bram Stoker",
    )
    .await;
    merge_books(&pool, &b, &c, None).await.unwrap();

    let c_id = book_id_by_uuid(&pool, &c).await;
    // Both absorbed uuids now point at C.
    let targets: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT book_id FROM merged_uuids")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(targets, vec![c_id]);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 2);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM book_files").await, 3);
}

#[tokio::test]
async fn reindex_diff_classifies_merged_source_file_as_unchanged() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    merge_books(&pool, &source, &target, None).await.unwrap();

    // The source file is still on disk with the same stat the indexer
    // recorded; the next audiobook reindex must classify it Unchanged.
    let ab = indexed_audiobook("B/Drakula.m4b", "Drakula", Some("Bram Stoker"));
    let db_rows = list_merged_rows_for_formats(&pool, "/audio", &["M4B", "M4A", "MP3"])
        .await
        .unwrap();
    let disk = vec![crate::ebook::StatEntry {
        filename: ab.group_path.clone(),
        scan_key: ab.scan_key.clone(),
        mtime_epoch: ab.max_mtime_epoch,
        size_bytes: ab.total_size_bytes,
        error: None,
    }];
    let diff = diff_library(&disk, &db_rows, std::path::Path::new("/audio"), true);
    // The merged row's identity in the diff is `merged_uuids.uuid` = the
    // source book's (now-deleted) uuid, which equals `source`.
    assert_eq!(diff.unchanged, vec![source]);
    assert!(diff.new.is_empty());
}

#[tokio::test]
async fn merged_two_epub_book_classifies_both_files_unchanged_on_rescan() {
    // Each book_files row must diff against its own stat, not a MAX() composite.
    let pool = init_db("sqlite::memory:").await.unwrap();
    crate::sync::sync_books(
        &pool,
        "/ebooks",
        crate::sync::SyncPlan {
            new_books: vec![crate::test_support::indexed_with_stat(
                "A/one.epub",
                Some("One"),
                1000,
                500,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let target = crate::test_support::uuid_by_scan_key(&pool, "A/one.epub").await;

    crate::sync::sync_books(
        &pool,
        "/ebooks",
        crate::sync::SyncPlan {
            new_books: vec![crate::test_support::indexed_with_stat(
                "B/two.epub",
                Some("Two"),
                2000,
                100,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let source = crate::test_support::uuid_by_scan_key(&pool, "B/two.epub").await;

    merge_books(&pool, &source, &target, None).await.unwrap();

    let disk = vec![
        crate::ebook::StatEntry {
            filename: "A/one.epub".into(),
            scan_key: "A/one.epub".into(),
            mtime_epoch: 1000,
            size_bytes: 500,
            error: None,
        },
        crate::ebook::StatEntry {
            filename: "B/two.epub".into(),
            scan_key: "B/two.epub".into(),
            mtime_epoch: 2000,
            size_bytes: 100,
            error: None,
        },
    ];
    let mut db_rows = list_indexed_rows_for_formats(&pool, "/ebooks", crate::ebook::EBOOK_FORMATS)
        .await
        .unwrap();
    db_rows.extend(
        list_merged_rows_for_formats(&pool, "/ebooks", crate::ebook::EBOOK_FORMATS)
            .await
            .unwrap(),
    );

    let diff = diff_library(&disk, &db_rows, std::path::Path::new("/ebooks"), true);
    assert_eq!(diff.unchanged.len(), 2, "{diff:?}");
    assert!(diff.unchanged.contains(&target));
    assert!(diff.unchanged.contains(&source));
    assert!(diff.new.is_empty(), "{:?}", diff.new);
    assert!(diff.changed.is_empty(), "{:?}", diff.changed);
}
