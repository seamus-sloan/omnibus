//! Unit tests for the `journals` module: create round-trip + markdown render,
//! `BookNotFound`, the public all-users list ordering, owner-scoped
//! update/delete (with non-owner `NotFound`), merged-uuid canonical
//! resolution, and the update/delete orphan-image-cleanup diffing.

use omnibus_shared::{CreateJournalEntry, EbookMetadata, JournalStatus, UpdateJournalEntry};

use super::*;
use crate::journal_images::{self, journal_images_dir};
use crate::test_support::EnvVarGuard;
use crate::{init_db, replace_books};

async fn seed(pool: &SqlitePool, library: &str, title: &str) -> String {
    replace_books(
        pool,
        library,
        vec![crate::ebook::IndexedBook {
            metadata: EbookMetadata {
                filename: format!("{title}.epub").to_lowercase(),
                title: Some(title.to_string()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
        }],
    )
    .await
    .expect("seed book");
    let books = crate::list_books(pool, library).await.unwrap();
    books
        .into_iter()
        .find(|b| b.title.as_deref() == Some(title))
        .unwrap()
        .unique_identifier
        .unwrap()
}

async fn seed_user(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, '!x', 0, 0, 0, 1) RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn seed_merged_uuid(pool: &SqlitePool, uuid: &str, book_uuid: &str, format: &str) {
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(book_uuid)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT OR REPLACE INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES (?, ?, ?, '/lib')",
    )
    .bind(uuid)
    .bind(book_id)
    .bind(format)
    .execute(pool)
    .await
    .expect("seed merged uuid");
}

fn create(uuid: &str, body: &str, progress: Option<u8>) -> CreateJournalEntry {
    CreateJournalEntry {
        book_uuid: uuid.to_string(),
        body_md: body.to_string(),
        progress,
        status: JournalStatus::default(),
    }
}

fn create_draft(uuid: &str, body: &str) -> CreateJournalEntry {
    CreateJournalEntry {
        status: JournalStatus::Draft,
        ..create(uuid, body, None)
    }
}

#[tokio::test]
async fn create_round_trips_and_renders_markdown() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;

    let saved = create_journal_entry(&pool, user, &create(&uuid, "**loved** it", Some(42)))
        .await
        .unwrap();
    assert_eq!(saved.book_uuid, uuid);
    assert_eq!(saved.author_id, user);
    assert_eq!(saved.author_name, "alice");
    assert_eq!(saved.body_md, "**loved** it");
    assert!(saved.body_html.contains("<strong>loved</strong>"));
    assert_eq!(saved.progress, Some(42));
}

#[tokio::test]
async fn create_returns_book_not_found_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let err = create_journal_entry(&pool, user, &create("no-such-book", "x", None))
        .await
        .unwrap_err();
    assert!(matches!(err, JournalError::BookNotFound));
}

#[tokio::test]
async fn list_returns_all_users_entries_newest_first() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let uuid = seed(&pool, "/lib", "Book A").await;

    create_journal_entry(&pool, alice, &create(&uuid, "alice first", None))
        .await
        .unwrap();
    create_journal_entry(&pool, bob, &create(&uuid, "bob second", None))
        .await
        .unwrap();

    let entries = list_journal_entries(&pool, alice, &uuid).await.unwrap();
    assert_eq!(
        entries.len(),
        2,
        "every user's published entries are public"
    );
    // Newest first (created_at DESC, id DESC) — bob inserted last.
    assert_eq!(entries[0].author_name, "bob");
    assert_eq!(entries[0].body_md, "bob second");
    assert_eq!(entries[1].author_name, "alice");
}

#[tokio::test]
async fn list_returns_empty_for_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert!(list_journal_entries(&pool, 1, "no-such-book")
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn update_by_owner_changes_body() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let entry = create_journal_entry(&pool, user, &create(&uuid, "draft", None))
        .await
        .unwrap();

    let updated = update_journal_entry(
        &pool,
        user,
        entry.id,
        &UpdateJournalEntry {
            body_md: "*revised*".into(),
            progress: Some(90),
            status: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(updated.body_md, "*revised*");
    assert!(updated.body_html.contains("<em>revised</em>"));
    assert_eq!(updated.progress, Some(90));
}

#[tokio::test]
async fn update_by_non_owner_returns_not_found() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let entry = create_journal_entry(&pool, alice, &create(&uuid, "mine", None))
        .await
        .unwrap();

    let err = update_journal_entry(
        &pool,
        bob,
        entry.id,
        &UpdateJournalEntry {
            body_md: "hijack".into(),
            progress: None,
            status: None,
        },
    )
    .await
    .unwrap_err();
    assert!(matches!(err, JournalError::NotFound));
    // Untouched.
    let entries = list_journal_entries(&pool, alice, &uuid).await.unwrap();
    assert_eq!(entries[0].body_md, "mine");
}

#[tokio::test]
async fn delete_by_owner_removes_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let entry = create_journal_entry(&pool, user, &create(&uuid, "bye", None))
        .await
        .unwrap();

    delete_journal_entry(&pool, user, entry.id).await.unwrap();
    assert!(list_journal_entries(&pool, user, &uuid)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn delete_by_non_owner_returns_not_found() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let entry = create_journal_entry(&pool, alice, &create(&uuid, "mine", None))
        .await
        .unwrap();

    let err = delete_journal_entry(&pool, bob, entry.id)
        .await
        .unwrap_err();
    assert!(matches!(err, JournalError::NotFound));
    assert_eq!(
        list_journal_entries(&pool, alice, &uuid)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn create_resolves_merged_uuid_to_surviving_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let canonical = seed(&pool, "/lib", "Book A").await;
    seed_merged_uuid(&pool, "attached-uuid", &canonical, "EPUB").await;

    let saved = create_journal_entry(&pool, user, &create("attached-uuid", "note", None))
        .await
        .unwrap();
    assert_eq!(saved.book_uuid, canonical);
    // Readable via both the merged and the canonical uuid.
    assert_eq!(
        list_journal_entries(&pool, user, "attached-uuid")
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        list_journal_entries(&pool, user, &canonical)
            .await
            .unwrap()
            .len(),
        1
    );
}

/// Bulk-insert `count` entry rows for `book_uuid` without going through
/// `create_journal_entry` — too slow at over-cap row counts.
async fn seed_entries_raw(pool: &SqlitePool, user_id: i64, book_uuid: &str, count: i64) {
    sqlx::query(
        r"
        WITH RECURSIVE n(i) AS (
            SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO journal_entries (user_id, book_uuid, body_md, created_at)
        SELECT ?, ?, 'entry ' || i, i FROM n
        ",
    )
    .bind(count)
    .bind(user_id)
    .bind(book_uuid)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn list_journal_entries_caps_response_at_hard_limit() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let over_cap = LIST_JOURNAL_ENTRIES_LIMIT + 500;
    seed_entries_raw(&pool, user, &uuid, over_cap).await;

    let list = list_journal_entries(&pool, user, &uuid).await.unwrap();
    assert_eq!(
        list.len() as i64,
        LIST_JOURNAL_ENTRIES_LIMIT,
        "list_journal_entries must not return more than LIST_JOURNAL_ENTRIES_LIMIT rows",
    );
}

#[tokio::test]
async fn list_hides_drafts_from_other_viewers_but_shows_them_to_the_owner() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let uuid = seed(&pool, "/lib", "Book A").await;

    create_journal_entry(&pool, alice, &create(&uuid, "published note", None))
        .await
        .unwrap();
    let draft = create_journal_entry(&pool, alice, &create_draft(&uuid, "secret draft"))
        .await
        .unwrap();
    assert_eq!(draft.status, JournalStatus::Draft);

    let for_bob = list_journal_entries(&pool, bob, &uuid).await.unwrap();
    assert_eq!(for_bob.len(), 1, "bob must not see alice's draft");
    assert_eq!(for_bob[0].body_md, "published note");

    let for_alice = list_journal_entries(&pool, alice, &uuid).await.unwrap();
    assert_eq!(for_alice.len(), 2, "alice sees her own draft");
    assert!(for_alice
        .iter()
        .any(|e| e.status == JournalStatus::Draft && e.body_md == "secret draft"));
}

#[tokio::test]
async fn update_publishes_a_draft_and_none_keeps_the_stored_status() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let draft = create_journal_entry(&pool, alice, &create_draft(&uuid, "wip"))
        .await
        .unwrap();

    // `status: None` keeps the entry a draft.
    let still_draft = update_journal_entry(
        &pool,
        alice,
        draft.id,
        &UpdateJournalEntry {
            body_md: "wip more".into(),
            progress: None,
            status: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(still_draft.status, JournalStatus::Draft);
    assert!(list_journal_entries(&pool, bob, &uuid)
        .await
        .unwrap()
        .is_empty());

    // Publishing flips the status and surfaces the entry to everyone.
    let published = update_journal_entry(
        &pool,
        alice,
        draft.id,
        &UpdateJournalEntry {
            body_md: "done".into(),
            progress: None,
            status: Some(JournalStatus::Published),
        },
    )
    .await
    .unwrap();
    assert_eq!(published.status, JournalStatus::Published);
    let for_bob = list_journal_entries(&pool, bob, &uuid).await.unwrap();
    assert_eq!(for_bob.len(), 1);
    assert_eq!(for_bob[0].body_md, "done");
}

#[tokio::test]
async fn create_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    pool.close().await;

    let err = create_journal_entry(&pool, user, &create(&uuid, "x", None))
        .await
        .unwrap_err();
    assert!(matches!(err, JournalError::Sqlx(_)));
}

/// Write a real journal image file and return its `IMAGE_URL_PREFIX`-embedded
/// markdown reference plus its bare serving name. Caller must hold an
/// `OMNIBUS_JOURNAL_IMAGES_DIR` `EnvVarGuard` pointed at a temp dir.
fn write_image() -> (String, String) {
    let name = journal_images::write_journal_image("image/png", b"bytes").unwrap();
    let embed = format!("![img]({}{name})", markdown::IMAGE_URL_PREFIX);
    (embed, name)
}

#[tokio::test]
async fn update_deletes_an_image_dropped_from_the_body_but_keeps_one_still_referenced() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_JOURNAL_IMAGES_DIR", Some(tmp.path().as_os_str()));
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let (dropped_embed, dropped_name) = write_image();
    let (kept_embed, kept_name) = write_image();
    let entry = create_journal_entry(
        &pool,
        user,
        &create(&uuid, &format!("{dropped_embed} {kept_embed}"), None),
    )
    .await
    .unwrap();

    update_journal_entry(
        &pool,
        user,
        entry.id,
        &UpdateJournalEntry {
            body_md: kept_embed,
            progress: None,
            status: None,
        },
    )
    .await
    .unwrap();

    assert!(
        !journal_images_dir().join(&dropped_name).exists(),
        "image removed from the body must be swept"
    );
    assert!(
        journal_images_dir().join(&kept_name).exists(),
        "image still referenced by the new body must survive"
    );
}

#[tokio::test]
async fn update_keeps_an_image_still_referenced_by_another_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_JOURNAL_IMAGES_DIR", Some(tmp.path().as_os_str()));
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let (shared_embed, shared_name) = write_image();
    let other = create_journal_entry(&pool, user, &create(&uuid, &shared_embed, None))
        .await
        .unwrap();
    let entry = create_journal_entry(&pool, user, &create(&uuid, &shared_embed, None))
        .await
        .unwrap();

    update_journal_entry(
        &pool,
        user,
        entry.id,
        &UpdateJournalEntry {
            body_md: "no images now".into(),
            progress: None,
            status: None,
        },
    )
    .await
    .unwrap();

    assert!(
        journal_images_dir().join(&shared_name).exists(),
        "image referenced by another surviving entry must not be deleted"
    );
    // Sanity: the other entry is untouched.
    assert_eq!(
        get_entry_by_id(&pool, other.id).await.unwrap().body_md,
        shared_embed
    );
}

#[tokio::test]
async fn delete_removes_an_image_uniquely_referenced_by_the_deleted_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_JOURNAL_IMAGES_DIR", Some(tmp.path().as_os_str()));
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let (embed, name) = write_image();
    let entry = create_journal_entry(&pool, user, &create(&uuid, &embed, None))
        .await
        .unwrap();

    delete_journal_entry(&pool, user, entry.id).await.unwrap();

    assert!(
        !journal_images_dir().join(&name).exists(),
        "image uniquely referenced by the deleted entry must be swept"
    );
}

#[tokio::test]
async fn delete_keeps_an_image_still_referenced_by_another_entry() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_JOURNAL_IMAGES_DIR", Some(tmp.path().as_os_str()));
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let (shared_embed, shared_name) = write_image();
    create_journal_entry(&pool, user, &create(&uuid, &shared_embed, None))
        .await
        .unwrap();
    let entry = create_journal_entry(&pool, user, &create(&uuid, &shared_embed, None))
        .await
        .unwrap();

    delete_journal_entry(&pool, user, entry.id).await.unwrap();

    assert!(
        journal_images_dir().join(&shared_name).exists(),
        "image referenced by another surviving entry must not be deleted"
    );
}

#[tokio::test]
async fn cleanup_ignores_a_bare_name_substring_that_is_not_a_real_embed_reference() {
    let tmp = tempfile::tempdir().unwrap();
    let _env = EnvVarGuard::set_os("OMNIBUS_JOURNAL_IMAGES_DIR", Some(tmp.path().as_os_str()));
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "alice").await;
    let uuid = seed(&pool, "/lib", "Book A").await;
    let (embed, name) = write_image();
    let entry = create_journal_entry(&pool, user, &create(&uuid, &embed, None))
        .await
        .unwrap();
    // A second entry's body happens to contain the bare serving name as a
    // substring (e.g. quoted in prose), without the `IMAGE_URL_PREFIX` that
    // makes it a real embed. A bare-name `LIKE %name%` match would wrongly
    // count this as "still referenced" and skip the sweep.
    create_journal_entry(
        &pool,
        user,
        &create(&uuid, &format!("mentions the file {name} in passing"), None),
    )
    .await
    .unwrap();

    delete_journal_entry(&pool, user, entry.id).await.unwrap();

    assert!(
        !journal_images_dir().join(&name).exists(),
        "a bare-name substring elsewhere must not block the sweep of a true orphan"
    );
}
