//! Tests for the missing-files GC: the `gc_books_missing_files` purge predicate
//! (retention window, user-data / attachment / override guards), the boot
//! backfill, and the sync-wired lifecycle (a removed file flags the row, a
//! returning file clears it, identity + overrides survive a repoint).

use omnibus_shared::metadata_lookup::{ExternalBookMeta, MetadataProvider};
use omnibus_shared::physical::WishlistSource;
use omnibus_shared::shelves::{CreateShelfRequest, ShelfKind};
use omnibus_shared::{MetadataOverrides, Settings};
use sqlx::SqlitePool;

use super::*;
use crate::auth::create_user;
use crate::covers::find_override_cover_file;
use crate::list_books;
use crate::metadata_overrides::{
    get_metadata_overrides, upsert_metadata_overrides, write_override_cover,
};
use crate::pool::init_db;
use crate::settings::set_settings;
use crate::sync::{replace_books, sync_books, SyncPlan};
use crate::test_support::{indexed, indexed_with_stat, uuid_by_scan_key, CoversTempDir};

/// Seed one book under `/lib`, then make it missing via the Removed bucket —
/// the real path that stamps `is_missing_files` / `missing_files_since`.
async fn seed_and_make_missing(pool: &SqlitePool, file: &str) -> String {
    replace_books(
        pool,
        "/lib",
        vec![indexed(file, Some("T"), &["A"], &[], None, None)],
    )
    .await
    .unwrap();
    let uuid = uuid_by_scan_key(pool, file).await;
    sync_books(
        pool,
        "/lib",
        SyncPlan {
            removed_uuids: vec![uuid.clone()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    uuid
}

/// Backdate a missing row's clock so it falls outside the retention window.
async fn backdate_missing_since(pool: &SqlitePool, uuid: &str, days_ago: i64) {
    let sql = format!(
        "UPDATE books SET missing_files_since = unixepoch('now', '-{days_ago} days') WHERE uuid = ?"
    );
    sqlx::query(&sql).bind(uuid).execute(pool).await.unwrap();
}

async fn book_exists(pool: &SqlitePool, uuid: &str) -> bool {
    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books WHERE uuid = ?")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap();
    n == 1
}

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
        "a wishlisted book is never purged"
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
        "a book on a hand-picked shelf is never purged"
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

#[tokio::test]
async fn gc_keeps_book_with_files() {
    let _covers = CoversTempDir::new("gc_has_file");
    let pool = init_db("sqlite::memory:").await.unwrap();
    // A live book (has a book_files row) is not missing, so the GC ignores it
    // even if its flag were somehow stale.
    replace_books(
        &pool,
        "/lib",
        vec![indexed("live.epub", Some("Live"), &["A"], &[], None, None)],
    )
    .await
    .unwrap();
    let uuid = uuid_by_scan_key(&pool, "live.epub").await;
    // Force a stale flag to prove the NOT EXISTS book_files guard wins.
    sqlx::query("UPDATE books SET is_missing_files = 1, missing_files_since = unixepoch('now','-99 days') WHERE uuid = ?")
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(book_exists(&pool, &uuid).await, "a book with files is kept");
}

#[tokio::test]
async fn gc_keeps_book_with_merged_uuid_attachment() {
    let _covers = CoversTempDir::new("gc_attachment");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_and_make_missing(&pool, "anchor.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES ('attached-uuid', ?, 'EPUB', '/lib')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 0);
    assert!(
        book_exists(&pool, &uuid).await,
        "a book still anchoring a cross-format attachment is kept"
    );
}

#[tokio::test]
async fn gc_keeps_book_with_missing_files_override_set() {
    let _covers = CoversTempDir::new("gc_override_exempt");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_and_make_missing(&pool, "wishlist.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    // The manual escape hatch: nothing in production sets the override yet —
    // wishlist/shelf books are protected by their own guards, not this flag.
    sqlx::query("UPDATE books SET is_missing_files_override = 1 WHERE uuid = ?")
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
        "an intentionally-fileless book is never purged"
    );
}

#[tokio::test]
async fn gc_deletes_override_row_and_cover_file_for_purged_book() {
    let _covers = CoversTempDir::new("gc_override_cover");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    let uuid = seed_and_make_missing(&pool, "edited.epub").await;
    let ov = MetadataOverrides {
        title: Some("Hand Edited".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, true, user_id)
        .await
        .unwrap();
    write_override_cover(&uuid, "image/png", b"OVERRIDE").unwrap();
    backdate_missing_since(&pool, &uuid, 40).await;

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 1);
    assert!(!book_exists(&pool, &uuid).await);
    assert!(
        get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .is_none(),
        "override row removed with the purged book"
    );
    assert!(
        find_override_cover_file(&uuid).is_none(),
        "override cover file unlinked"
    );
}

#[tokio::test]
async fn gc_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap_err();
    assert!(matches!(err, MissingFilesError::Db(_)));
}

#[tokio::test]
async fn backfill_flags_existing_fileless_books_and_is_idempotent() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    // Pre-migration fileless book (no book_files row).
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) VALUES ('fileless', 1, '', 'G')",
    )
    .execute(&pool)
    .await
    .unwrap();
    // A live book with a file must stay un-flagged.
    sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES ('live', 1, '', 'L')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         SELECT id, 'EPUB', 'L', 1, 1 FROM books WHERE uuid = 'live'",
    )
    .execute(&pool)
    .await
    .unwrap();

    backfill_missing_files_flags(&pool).await.unwrap();
    let (flag, since): (i64, Option<i64>) = sqlx::query_as(
        "SELECT is_missing_files, missing_files_since FROM books WHERE uuid='fileless'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(flag, 1);
    let first_since = since.expect("missing_files_since stamped");
    let live_flag: i64 = sqlx::query_scalar("SELECT is_missing_files FROM books WHERE uuid='live'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(live_flag, 0, "a book with a file is not flagged");

    // Second run is a no-op — the IS NULL/= 0 guard keeps `since` stable.
    backfill_missing_files_flags(&pool).await.unwrap();
    let since2: Option<i64> =
        sqlx::query_scalar("SELECT missing_files_since FROM books WHERE uuid='fileless'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(since2, Some(first_since), "idempotent: clock unchanged");
}

#[tokio::test]
async fn removed_file_sets_is_missing_files_flag_and_since() {
    let _covers = CoversTempDir::new("flag_set");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_and_make_missing(&pool, "a.epub").await;
    let (flag, since): (i64, Option<i64>) =
        sqlx::query_as("SELECT is_missing_files, missing_files_since FROM books WHERE uuid = ?")
            .bind(&uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(flag, 1);
    assert!(since.is_some(), "retention clock started");
}

#[tokio::test]
async fn returning_file_clears_flag_and_preserves_override() {
    let _covers = CoversTempDir::new("lose_regain");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    // Index, hand-edit, then lose the file.
    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
    )
    .await
    .unwrap();
    let uuid = uuid_by_scan_key(&pool, "a.epub").await;
    let ov = MetadataOverrides {
        title: Some("Hand Edited".into()),
        ..Default::default()
    };
    upsert_metadata_overrides(&pool, &uuid, &ov, false, user_id)
        .await
        .unwrap();
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            removed_uuids: vec![uuid.clone()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let flag: i64 = sqlx::query_scalar("SELECT is_missing_files FROM books WHERE uuid = ?")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(flag, 1, "missing while the file is gone");

    // File returns → flag cleared, same uuid, override preserved.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            new_books: vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (flag, since): (i64, Option<i64>) =
        sqlx::query_as("SELECT is_missing_files, missing_files_since FROM books WHERE uuid = ?")
            .bind(&uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(flag, 0, "flag cleared on return");
    assert!(since.is_none(), "retention clock cleared on return");
    let after = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(after.len(), 1);
    assert_eq!(after[0].unique_identifier.as_deref(), Some(uuid.as_str()));
    assert_eq!(
        get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .unwrap()
            .0
            .title,
        Some("Hand Edited".into()),
        "override survived the lose→regain cycle"
    );
}

#[tokio::test]
async fn library_repoint_round_trip_preserves_identity_and_override() {
    let _covers = CoversTempDir::new("repoint_rt");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
    )
    .await
    .unwrap();
    let uuid = uuid_by_scan_key(&pool, "a.epub").await;
    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Hand Edited".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();
    let only = |p: &str| Settings {
        ebook_library_path: Some(p.to_string()),
        audiobook_library_path: None,
        scan_interval_hours: None,
    };
    // Record the current root, then repoint it to a new directory.
    set_settings(&pool, &only("/lib")).await.unwrap();
    set_settings(&pool, &only("/lib2")).await.unwrap();

    let after = list_books(&pool, "/lib2").await.unwrap();
    assert_eq!(after.len(), 1, "book lists under the repointed root");
    assert_eq!(after[0].unique_identifier.as_deref(), Some(uuid.as_str()));
    let flag: i64 = sqlx::query_scalar("SELECT is_missing_files FROM books WHERE uuid = ?")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(flag, 0, "a repoint never flags the book missing");

    // Round-trip back to the original root — still the same book + override.
    set_settings(&pool, &only("/lib")).await.unwrap();
    let back = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].unique_identifier.as_deref(), Some(uuid.as_str()));
    assert!(
        get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .is_some(),
        "override survived the repoint round-trip"
    );
}

/// Documents the path-based-identity limitation (to revisit under F2 content
/// identity): a book is identified by `(library slot, relative scan_key)`, not
/// file content. Repointing a slot to a directory whose `alpha.epub` sits at
/// the same relative path adopts the prior book's identity — so the override
/// persists (smears onto the new physical file) and there is **one** book, not
/// two with metadata lost.
#[tokio::test]
async fn identical_relative_path_in_repointed_directory_keeps_one_book_and_preserves_override() {
    let _covers = CoversTempDir::new("alpha_alpha");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    // Dir A: alpha.epub, hand-edited.
    replace_books(
        &pool,
        "/lib",
        vec![indexed_with_stat("alpha.epub", Some("Alpha A"), 100, 100)],
    )
    .await
    .unwrap();
    let uuid = uuid_by_scan_key(&pool, "alpha.epub").await;
    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Hand Edited".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    // Repoint to dir B whose *different* alpha.epub sits at the same relative
    // path → a content change to the SAME book (uuid preserved).
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            changed_books: vec![indexed_with_stat("alpha.epub", Some("Alpha B"), 200, 200)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let mid = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(mid.len(), 1, "one book, not two");
    assert_eq!(mid[0].unique_identifier.as_deref(), Some(uuid.as_str()));
    assert!(
        get_metadata_overrides(&pool, &uuid)
            .await
            .unwrap()
            .is_some(),
        "override smears onto the same-path file (known path-identity limit)"
    );

    // Repoint back to dir A — still one book, same uuid, override intact.
    sync_books(
        &pool,
        "/lib",
        SyncPlan {
            changed_books: vec![indexed_with_stat("alpha.epub", Some("Alpha A"), 100, 100)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let back = list_books(&pool, "/lib").await.unwrap();
    assert_eq!(back.len(), 1);
    assert_eq!(back[0].unique_identifier.as_deref(), Some(uuid.as_str()));
    assert!(get_metadata_overrides(&pool, &uuid)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn backfill_skips_intentionally_fileless_override_rows() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    // An intentionally-fileless row (override = 1) must stay un-flagged so
    // reads keep showing it.
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, is_missing_files_override)
         VALUES ('wish', 1, '', 'W', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();

    backfill_missing_files_flags(&pool).await.unwrap();

    let (flag, since): (i64, Option<i64>) = sqlx::query_as(
        "SELECT is_missing_files, missing_files_since FROM books WHERE uuid = 'wish'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(flag, 0, "wishlist row is never flagged missing");
    assert!(since.is_none());
}

#[tokio::test]
async fn fileless_book_still_appears_in_author_browse() {
    let _covers = CoversTempDir::new("fileless_browse");
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Seeded with author "A"; then its file goes missing.
    seed_and_make_missing(&pool, "gone.epub").await;

    // The grid hides a fileless book (its own EXISTS book_files filter)…
    assert!(
        list_books(&pool, "/lib").await.unwrap().is_empty(),
        "the grid hides a fileless book"
    );
    // …but author browse still lists its author, with the book counted.
    let authors = crate::browse::list_authors(&pool, &["/lib"]).await.unwrap();
    assert_eq!(authors.len(), 1, "author still shown for a fileless book");
    assert_eq!(authors[0].book_count, 1);
}

#[tokio::test]
async fn gc_purges_orphan_author_when_its_last_book_is_deleted() {
    let _covers = CoversTempDir::new("gc_orphan_tax");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_and_make_missing(&pool, "solo.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;
    // The author survives while the (fileless) book still references it.
    let before: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(before, 1);

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 1);
    let after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(after, 0, "the purged book's now-bookless author is GC'd");
}

/// The GC's `NOT EXISTS ... WHERE book_uuid = b.uuid` subqueries need a
/// `book_uuid`-first index on each table — pre-existing composites like
/// `(user_id, book_uuid)` or the `(shelf_id, book_uuid)` primary key can't
/// serve the probe because SQLite skips a multi-column index when the leading
/// column isn't in the predicate. Assert every table's plan uses its
/// book-uuid covering index (a `SEARCH` — a `SCAN` would mean the migration
/// was lost or the index name drifted). Same shape as the GC's per-table
/// subquery.
#[tokio::test]
async fn user_data_book_uuid_probes_use_the_book_uuid_first_index() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let cases = [
        ("reading_progress", "idx_reading_progress_book_uuid"),
        ("bookmarks", "idx_bookmarks_book_uuid"),
        ("reading_sessions", "idx_reading_sessions_book_uuid"),
        ("listening_sessions", "idx_listening_sessions_book_uuid"),
        ("annotations", "idx_annotations_book_uuid"),
        ("wishlist_entries", "idx_wishlist_book"),
        ("shelf_books", "idx_shelf_books_book_uuid"),
    ];
    for (table, index) in cases {
        let sql = format!("EXPLAIN QUERY PLAN SELECT 1 FROM {table} WHERE book_uuid = ?");
        let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(&sql)
            .bind("any-uuid")
            .fetch_all(&pool)
            .await
            .unwrap();
        let text = plan
            .iter()
            .map(|(_, _, _, s)| s.as_str())
            .collect::<Vec<_>>()
            .join(" | ");
        assert!(
            text.contains(&format!("SEARCH {table}")) && text.contains(index),
            "expected {table} probe to SEARCH via {index}, got: {text}"
        );
    }
}

#[tokio::test]
async fn gc_deletes_the_purged_books_kobo_annotation_sync_state() {
    // A per-device annotation watermark (#1278) is bookkeeping, not user
    // data — it must not guard a victim, and it must not survive the purge:
    // an orphaned row would make Reading Services `checkforchanges`
    // re-report the dead uuid forever (its GET can only 404, never ack).
    let _covers = CoversTempDir::new("gc_kobo_sync");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_and_make_missing(&pool, "gone-kobo.epub").await;
    backdate_missing_since(&pool, &uuid, 40).await;

    let user_id = create_user(&pool, "kobo-reader", "securepassword1")
        .await
        .unwrap()
        .id;
    let device = crate::kobo_devices::create_device(&pool, user_id, "Kobo")
        .await
        .unwrap();
    crate::kobo::annotations::mark_adopted(&pool, device.id, &uuid)
        .await
        .unwrap();

    let purged = gc_books_missing_files(&pool, MISSING_FILES_RETENTION_DAYS)
        .await
        .unwrap();
    assert_eq!(purged, 1, "sync state alone must not guard a victim");

    let sync_rows: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM kobo_annotations_sync WHERE book_uuid = ?")
            .bind(&uuid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        sync_rows, 0,
        "orphaned watermark rows must purge with the book"
    );
}
