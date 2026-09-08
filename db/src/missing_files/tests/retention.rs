//! The purge predicate's retention window and its user-data guards: a
//! ghosted book past retention is purged unless it carries progress, a
//! physical copy, a wishlist or manual-shelf membership, a playback
//! preference, a rating, ledger rows or a journal entry.

use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider};
use omnibus_shared::physical::WishlistSource;
use omnibus_shared::shelves::{CreateShelfRequest, ShelfKind};

use super::super::*;
use super::{backdate_missing_since, book_exists, seed_and_make_missing};
use crate::auth::create_user;
use crate::pool::init_db;
use crate::test_support::CoversTempDir;

#[tokio::test]
async fn gc_purges_book_missing_files_past_retention_with_no_user_data() {
    let _covers = CoversTempDir::new("gc_acceptance");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_and_make_missing(&pool, "gone.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 1);
    assert!(!book_exists(&pool, &uuid).await, "row purged");
}

#[tokio::test]
async fn gc_keeps_book_within_retention_window() {
    let _covers = CoversTempDir::new("gc_within_window");
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Freshly missing (missing_files_since = now) → inside the 30-day window.
    let uuid = seed_and_make_missing(&pool, "fresh.epub").await;

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(book_exists(&pool, &uuid).await, "recent miss is retained");
}

#[tokio::test]
async fn gc_keeps_book_with_reading_progress() {
    let _covers = CoversTempDir::new("gc_user_data");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = seed_and_make_missing(&pool, "read.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    sqlx::query(
        "INSERT INTO reading_progress (user_id, book_uuid, format, epub_cfi)
         VALUES (?, ?, 'epub', 'epubcfi(/6/2)')",
    )
    .bind(user_id)
    .bind(&uuid)
    .execute(&pool)
    .await
    .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(
        book_exists(&pool, &uuid).await,
        "a book a user has read is never purged"
    );
}

#[tokio::test]
async fn gc_keeps_book_with_physical_copy() {
    let _covers = CoversTempDir::new("gc_physical");
    let pool = init_db("sqlite::memory:").await.unwrap();
    // A book whose digital file was removed (flagged missing, past retention)
    // but which now has a checked-in physical copy must survive the GC —
    // otherwise the copy is orphaned and the physical-only book vanishes.
    let uuid = seed_and_make_missing(&pool, "sold_digital.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    crate::physical::add_physical_copy(&pool, &uuid, None, None, None)
        .await
        .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(
        book_exists(&pool, &uuid).await,
        "a book with a physical copy is never purged"
    );
}

#[tokio::test]
async fn gc_keeps_book_on_a_wishlist_and_purges_after_the_entry_is_removed() {
    let _covers = CoversTempDir::new("gc_wishlist");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = seed_and_make_missing(&pool, "wanted.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    crate::physical::add_wishlist_entry(&pool, user_id, &uuid, WishlistSource::Manual)
        .await
        .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(
        book_exists(&pool, &uuid).await,
        "a book is kept while it is on a wishlist"
    );

    // Removing the last entry lifts the guard; the still-running clock makes
    // the book eligible on the next sweep.
    crate::physical::remove_wishlist_entry(&pool, user_id, &uuid)
        .await
        .unwrap();
    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 1);
    assert!(!book_exists(&pool, &uuid).await);
}

#[tokio::test]
async fn gc_keeps_book_on_a_manual_shelf_past_retention() {
    let _covers = CoversTempDir::new("gc_shelf");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = seed_and_make_missing(&pool, "shelved.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    crate::shelves::create_shelf(
        &pool,
        user_id,
        &CreateShelfRequest {
            kind: ShelfKind::Manual,
            name: "Keepers".into(),
            description: None,
            visibility: Default::default(),
            match_mode: None,
            rules: vec![],
            book_uuids: vec![uuid.clone()],
        },
    )
    .await
    .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(
        book_exists(&pool, &uuid).await,
        "a book is kept while on a hand-picked shelf"
    );
}

#[tokio::test]
async fn gc_keeps_fileless_wishlist_book_stamped_by_boot_backfill() {
    let _covers = CoversTempDir::new("gc_wishlist_fileless");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    // The real path: scanning an ISBN for a book not in the library and
    // wishlisting it mints a fileless book with no override flag set.
    let meta = ExternalBookMeta {
        isbn13: "9780306406157".into(),
        title: "Wanted in Print".into(),
        authors: vec!["A. Author".into()],
        year: None,
        pages: None,
        publisher: None,
        description: None,
        cover_url: None,
        series: None,
        first_publish_year: None,
        source: MetadataProvider::OpenLibrary,
    };
    let uuid = crate::scan::wishlist_add(&pool, user_id, None, Some(&meta), WishlistSource::Scan)
        .await
        .unwrap();

    // The next boot stamps it missing and starts the clock; backdate it past
    // the retention window to prove the wishlist guard alone keeps it.
    backfill_missing_files_flags(&pool).await.unwrap();
    backdate_missing_since(&pool, &uuid, 40).await;

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(
        book_exists(&pool, &uuid).await,
        "a wishlist-only fileless book survives the GC"
    );
}

#[tokio::test]
async fn gc_keeps_book_with_audiobook_playback_preference() {
    let _covers = CoversTempDir::new("gc_playback_preference");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = seed_and_make_missing(&pool, "listened.m4b").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    sqlx::query(
        "INSERT INTO audiobook_playback_preferences
            (user_id, book_uuid, playback_rate)
         VALUES (?, ?, 1.5)",
    )
    .bind(user_id)
    .bind(&uuid)
    .execute(&pool)
    .await
    .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(
        book_exists(&pool, &uuid).await,
        "a book with a playback preference is never purged"
    );
}

#[tokio::test]
async fn gc_keeps_book_with_user_rating() {
    let _covers = CoversTempDir::new("gc_user_rating");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = seed_and_make_missing(&pool, "rated.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    sqlx::query("INSERT INTO user_ratings (user_id, book_uuid, half_stars) VALUES (?, ?, 9)")
        .bind(user_id)
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(
        book_exists(&pool, &uuid).await,
        "a book a user has rated is never purged"
    );
}

#[tokio::test]
async fn gc_keeps_book_with_forward_progress_ledger() {
    // #2139: the day buckets are reading history, so a book that still records
    // ground covered must survive the purge — the guard is what stops the GC
    // deleting the `books` row those rows join against.
    let _covers = CoversTempDir::new("gc_pages_ledger");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = seed_and_make_missing(&pool, "read.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    sqlx::query(
        "INSERT INTO reading_progress_daily
             (user_id, book_uuid, format, day, percent_gained)
         VALUES (?, ?, 'epub', '2026-08-01', 12)",
    )
    .bind(user_id)
    .bind(&uuid)
    .execute(&pool)
    .await
    .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(
        book_exists(&pool, &uuid).await,
        "a book with recorded page progress is never purged"
    );
}

#[tokio::test]
async fn gc_keeps_book_with_journal_entry() {
    let _covers = CoversTempDir::new("gc_user_journal");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = seed_and_make_missing(&pool, "journaled.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    sqlx::query(
        "INSERT INTO journal_entries (user_id, book_uuid, body_md) VALUES (?, ?, 'a note')",
    )
    .bind(user_id)
    .bind(&uuid)
    .execute(&pool)
    .await
    .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(
        book_exists(&pool, &uuid).await,
        "a book with a journal entry is never purged"
    );
}
