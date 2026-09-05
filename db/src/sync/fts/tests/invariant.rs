//! The no-orphan invariant after every public write path (`sync_books`,
//! `replace_books`, `sync_audiobooks`), the cross-format attach gap
//! regression, and the single-row `upsert_fts` / `delete_fts` round trip.

use super::super::*;
use super::{assert_fts_invariant, fts_isbn_hits, indexed_with_isbn};
use crate::pool::init_db;
use crate::sync::{sync_audiobooks, sync_books, AudiobookSyncPlan, SyncPlan};
use crate::test_support::{count_rows, indexed, indexed_audiobook, CoversTempDir};

#[tokio::test]
async fn fts_invariant_holds_after_sync_books_new_changed_and_removed() {
    let _covers = CoversTempDir::new("fts_invariant_ebooks");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // New.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![
                indexed("a.epub", Some("Alpha"), &["Ann"], &["sci-fi"], None, None),
                indexed("b.epub", Some("Beta"), &["Bob"], &[], None, None),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 2);
    assert_fts_invariant(&pool).await;

    // Changed (same filename, new title) — preserves id, refreshes FTS.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            changed_books: vec![indexed(
                "a.epub",
                Some("Alpha Prime"),
                &["Ann"],
                &["sci-fi"],
                None,
                None,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_fts_invariant(&pool).await;

    // Removed → fileless (F2). Identity is minted, so resolve b.epub's uuid
    // by scan_key, then assert it is retained as a fileless book that keeps
    // its FTS row (2 books total, 1 file-backed).
    let b_uuid = crate::test_support::uuid_by_scan_key(&pool, "b.epub").await;
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            removed_uuids: vec![b_uuid],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        2,
        "removed book retained as a fileless book"
    );
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM books b
              WHERE EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)"
        )
        .await,
        1,
        "only the surviving book is file-backed"
    );
    assert_fts_invariant(&pool).await;
}

#[tokio::test]
async fn fts_invariant_holds_after_replace_books() {
    let _covers = CoversTempDir::new("fts_invariant_replace");
    let pool = init_db("sqlite::memory:").await.unwrap();
    crate::sync::replace_books(
        &pool,
        "/lib",
        vec![
            indexed("x.epub", Some("Ex"), &["Xi"], &[], None, None),
            indexed("y.epub", Some("Why"), &["Yi"], &[], None, None),
        ],
    )
    .await
    .unwrap();
    assert_fts_invariant(&pool).await;

    // Replace again with one fewer book — the dropped book's FTS row must go,
    // but the book is retained as a fileless book (F2): 2 books total, 1
    // file-backed (and therefore 1 FTS row).
    crate::sync::replace_books(
        &pool,
        "/lib",
        vec![indexed("x.epub", Some("Ex"), &["Xi"], &[], None, None)],
    )
    .await
    .unwrap();
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 2);
    assert_eq!(
        count_rows(
            &pool,
            "SELECT COUNT(*) FROM books b
              WHERE EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)"
        )
        .await,
        1
    );
    assert_fts_invariant(&pool).await;
}

#[tokio::test]
async fn fts_invariant_holds_after_sync_audiobooks_new_changed_and_removed() {
    let _covers = CoversTempDir::new("fts_invariant_audiobooks");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let ab = indexed_audiobook("Stoker/Dracula", "Dracula", Some("Bram Stoker"));
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![ab],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_fts_invariant(&pool).await;
    // Identity is minted (F2) — read the durable uuid back by scan_key.
    let uuid = crate::test_support::uuid_by_scan_key(&pool, "Stoker/Dracula").await;

    // Changed — matched by scan_key (the group path), identity preserved.
    let changed = indexed_audiobook(
        "Stoker/Dracula",
        "Dracula (Unabridged)",
        Some("Bram Stoker"),
    );
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            changed_books: vec![changed],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_fts_invariant(&pool).await;

    // Removed → fileless (F2): the books row is retained, its book_files row
    // is gone, but its links + FTS row stay (it remains searchable).
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            removed_uuids: vec![uuid],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "removed book is retained as a fileless book"
    );
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        0
    );
    assert_fts_invariant(&pool).await;
}

#[tokio::test]
async fn attaching_second_ebook_format_makes_its_new_isbn_searchable() {
    // Regression for the attach gap: a second format attaching to an
    // existing book unions its identifiers (incl. ISBN), but the pre-fix
    // code never refreshed the target's FTS row — so the ISBN wasn't
    // searchable. This test fails on the old code and passes after the
    // door is called from `attach_ebook_file`.
    let _covers = CoversTempDir::new("fts_attach_ebook_isbn");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // Seed an EPUB with no ISBN.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![indexed(
                "Dracula.epub",
                Some("Dracula"),
                &["Bram Stoker"],
                &[],
                None,
                None,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(fts_isbn_hits(&pool, "9781111111111").await, 0);

    // Attach a second format (MOBI) for the same work carrying a NEW ISBN.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![indexed_with_isbn(
                "Dracula.mobi",
                "Dracula",
                "Bram Stoker",
                "9781111111111",
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Still one book (the MOBI attached, not a new row).
    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        2
    );
    // The unioned ISBN is now searchable, and the invariant still holds.
    assert_eq!(fts_isbn_hits(&pool, "9781111111111").await, 1);
    assert_fts_invariant(&pool).await;
}

#[tokio::test]
async fn attaching_audiobook_to_existing_ebook_keeps_single_fts_row() {
    // The audiobook attach path carries no identifiers, so there is no new
    // ISBN to surface — but it must still leave exactly one FTS row for the
    // target (no orphan, no duplicate) after routing through the door.
    let _covers = CoversTempDir::new("fts_attach_audiobook");
    let pool = init_db("sqlite::memory:").await.unwrap();

    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![indexed(
                "Dracula.epub",
                Some("Dracula"),
                &["Bram Stoker"],
                &[],
                None,
                None,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![indexed_audiobook(
                "Stoker/Dracula",
                "Dracula",
                Some("Bram Stoker"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(count_rows(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(
        count_rows(&pool, "SELECT COUNT(*) FROM book_files").await,
        2
    );
    assert_fts_invariant(&pool).await;
}

#[tokio::test]
async fn upsert_and_delete_fts_round_trip_a_single_row() {
    let _covers = CoversTempDir::new("fts_door_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![indexed("a.epub", Some("Alpha"), &["Ann"], &[], None, None)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let id: i64 = sqlx::query_scalar("SELECT id FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();

    // delete_fts removes the row; upsert_fts puts it back from canonical data.
    let mut conn = pool.acquire().await.unwrap();
    delete_fts(&mut conn, id).await.unwrap();
    let after_delete: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts WHERE rowid = ?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after_delete, 0);

    upsert_fts(&mut conn, id).await.unwrap();
    let after_upsert: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts WHERE rowid = ?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after_upsert, 1);
    // upsert is idempotent — a second call doesn't duplicate.
    upsert_fts(&mut conn, id).await.unwrap();
    let after_second: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts WHERE rowid = ?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after_second, 1);
}
