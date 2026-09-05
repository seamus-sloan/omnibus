//! The sync-wired lifecycle: the boot backfill flags existing fileless
//! books (idempotently, skipping intentional ones), a removed file sets
//! the flag and stamp, a returning file clears it, identity and overrides
//! survive a library repoint, and a fileless book still browses under its
//! author.

use omnibus_shared::{MetadataOverrides, Settings};

use super::super::*;
use super::seed_and_make_missing;
use crate::auth::create_user;
use crate::list_books;
use crate::metadata_overrides::{get_metadata_overrides, upsert_metadata_overrides};
use crate::pool::init_db;
use crate::settings::set_settings;
use crate::sync::{replace_books, sync_books, SyncPlan};
use crate::test_support::{indexed, indexed_with_stat, uuid_by_scan_key, CoversTempDir};

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
    assert_eq!(flag, 0, "override row is never flagged missing");
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
