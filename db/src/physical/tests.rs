//! Physical ownership data-layer tests: copies (multiple per book,
//! individually deletable), wishlist (per-user, idempotent), check-in
//! fulfillment across all users, and fileless book creation.

use omnibus_shared::physical::WishlistSource;
use sqlx::SqlitePool;

use crate::covers::cover_path_for;
use crate::test_support::{seed_minimal_books, CoversTempDir};

use super::*;

async fn pool() -> SqlitePool {
    crate::pool::init_db("sqlite::memory:").await.unwrap()
}

async fn seed_user(pool: &SqlitePool, username: &str) -> i64 {
    // Insert directly: `create_user` gates all but the first registration on the
    // registration-enabled setting, and these tests only need user rows for FKs.
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash) VALUES (?1, 'x') RETURNING id",
    )
    .bind(username)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn count(pool: &SqlitePool, sql: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(sql)
        .fetch_one(pool)
        .await
        .unwrap()
}

// --- physical copies -------------------------------------------------------

#[tokio::test]
async fn add_physical_copy_returns_the_checked_in_row() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;

    let copy = add_physical_copy(&pool, "uuid-1", Some("9780000000001"), None, Some("1st ed"))
        .await
        .unwrap();

    assert_eq!(copy.book_uuid, "uuid-1");
    assert_eq!(copy.isbn.as_deref(), Some("9780000000001"));
    assert_eq!(copy.note.as_deref(), Some("1st ed"));
    assert!(copy.checked_in_at > 0);
}

#[tokio::test]
async fn add_physical_copy_allows_multiple_copies_per_book() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;

    add_physical_copy(&pool, "uuid-1", Some("a"), None, None)
        .await
        .unwrap();
    add_physical_copy(&pool, "uuid-1", Some("b"), None, None)
        .await
        .unwrap();

    let copies = list_physical_copies(&pool, "uuid-1").await.unwrap();
    assert_eq!(copies.len(), 2);
}

#[tokio::test]
async fn add_physical_copy_errors_when_book_missing() {
    let pool = pool().await;
    let err = add_physical_copy(&pool, "nope", None, None, None)
        .await
        .unwrap_err();
    assert!(matches!(err, PhysicalError::BookNotFound));
}

#[tokio::test]
async fn add_physical_copy_resolves_a_merged_uuid() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'uuid-1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    // A uuid that was merged into uuid-1 must still resolve to the live book.
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path)
         VALUES ('old-uuid', ?1, 'EPUB', '/lib')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    // Checked in against the merged ledger key, the copy is stored under the
    // canonical (surviving) uuid — the value the rest of the system keys on.
    let copy = add_physical_copy(&pool, "old-uuid", None, None, None)
        .await
        .unwrap();
    assert_eq!(copy.book_uuid, "uuid-1");
    // And it's findable by either uuid, since the read path folds to canonical.
    assert_eq!(
        list_physical_copies(&pool, "old-uuid").await.unwrap().len(),
        1
    );
    assert_eq!(
        list_physical_copies(&pool, "uuid-1").await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn delete_physical_copy_removes_one_copy() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let keep = add_physical_copy(&pool, "uuid-1", None, None, None)
        .await
        .unwrap();
    let drop = add_physical_copy(&pool, "uuid-1", None, None, None)
        .await
        .unwrap();

    delete_physical_copy(&pool, drop.id).await.unwrap();

    let copies = list_physical_copies(&pool, "uuid-1").await.unwrap();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].id, keep.id);
}

#[tokio::test]
async fn delete_physical_copy_errors_when_missing() {
    let pool = pool().await;
    let err = delete_physical_copy(&pool, 999).await.unwrap_err();
    assert!(matches!(err, PhysicalError::CopyNotFound));
}

// --- wishlist --------------------------------------------------------------

#[tokio::test]
async fn add_wishlist_entry_returns_the_entry() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "reader").await;

    let entry = add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Scan)
        .await
        .unwrap();

    assert_eq!(entry.user_id, user);
    assert_eq!(entry.book_uuid, "uuid-1");
    assert_eq!(entry.source, WishlistSource::Scan);
}

#[tokio::test]
async fn add_wishlist_entry_errors_when_book_missing() {
    let pool = pool().await;
    let user = seed_user(&pool, "reader").await;
    let err = add_wishlist_entry(&pool, user, "nope", WishlistSource::Scan)
        .await
        .unwrap_err();
    assert!(matches!(err, PhysicalError::BookNotFound));
}

#[tokio::test]
async fn add_wishlist_entry_is_idempotent() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "reader").await;

    let first = add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Scan)
        .await
        .unwrap();
    // A second add keeps the original row (same id, original source).
    let second = add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Manual)
        .await
        .unwrap();

    assert_eq!(first.id, second.id);
    assert_eq!(second.source, WishlistSource::Scan);
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM wishlist_entries").await,
        1
    );
}

#[tokio::test]
async fn remove_wishlist_entry_deletes_and_is_a_noop_when_absent() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "reader").await;
    add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Detail)
        .await
        .unwrap();

    remove_wishlist_entry(&pool, user, "uuid-1").await.unwrap();
    assert!(list_wishlist(&pool, user).await.unwrap().is_empty());

    // Removing again is not an error.
    remove_wishlist_entry(&pool, user, "uuid-1").await.unwrap();
}

/// Raw bulk insert bypassing `add_wishlist_entry` — the CRUD helper resolves
/// the book uuid on every call, too slow for a 1500-row response-cap
/// fixture. `wishlist_entries` uniques on `(user_id, book_uuid)`, so each
/// row needs a distinct synthetic uuid; the soft-reference (no FK) means
/// these need not resolve to real `books` rows for this query-shape test.
async fn seed_wishlist_raw(pool: &SqlitePool, user_id: i64, count: i64) {
    sqlx::query(
        r"
        WITH RECURSIVE n(i) AS (
            SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO wishlist_entries (user_id, book_uuid, added_at, source)
        SELECT ?, 'seed-uuid-' || i, i, 'manual' FROM n
        ",
    )
    .bind(count)
    .bind(user_id)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn list_wishlist_caps_response_at_hard_limit() {
    let pool = pool().await;
    let user = seed_user(&pool, "alice").await;
    let over_cap = LIST_WISHLIST_LIMIT + 500;
    seed_wishlist_raw(&pool, user, over_cap).await;

    let list = list_wishlist(&pool, user).await.unwrap();
    assert_eq!(
        list.len() as i64,
        LIST_WISHLIST_LIMIT,
        "list_wishlist must not return more than LIST_WISHLIST_LIMIT rows",
    );
}

#[tokio::test]
async fn list_wishlist_is_scoped_per_user() {
    let pool = pool().await;
    seed_minimal_books(&pool, 2).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    add_wishlist_entry(&pool, alice, "uuid-1", WishlistSource::Scan)
        .await
        .unwrap();
    add_wishlist_entry(&pool, bob, "uuid-2", WishlistSource::Scan)
        .await
        .unwrap();

    let alice_list = list_wishlist(&pool, alice).await.unwrap();
    assert_eq!(alice_list.len(), 1);
    assert_eq!(alice_list[0].book_uuid, "uuid-1");
}

// --- fulfillment (AC3) -----------------------------------------------------

#[tokio::test]
async fn add_physical_copy_fulfills_every_users_wishlist() {
    let pool = pool().await;
    seed_minimal_books(&pool, 2).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    add_wishlist_entry(&pool, alice, "uuid-1", WishlistSource::Scan)
        .await
        .unwrap();
    add_wishlist_entry(&pool, bob, "uuid-1", WishlistSource::Detail)
        .await
        .unwrap();
    // An unrelated wishlist entry must survive.
    add_wishlist_entry(&pool, alice, "uuid-2", WishlistSource::Scan)
        .await
        .unwrap();

    add_physical_copy(&pool, "uuid-1", None, None, None)
        .await
        .unwrap();

    assert!(list_wishlist(&pool, bob).await.unwrap().is_empty());
    let alice_left = list_wishlist(&pool, alice).await.unwrap();
    assert_eq!(alice_left.len(), 1);
    assert_eq!(alice_left[0].book_uuid, "uuid-2");
}

// --- fileless book (AC4) ---------------------------------------------------

#[tokio::test]
async fn create_fileless_book_makes_a_uuid_row_with_identifier_and_no_files() {
    let _covers = CoversTempDir::new("fileless");
    let pool = pool().await;

    let uuid = create_fileless_book(
        &pool,
        FilelessBook {
            title: "Physical Only".into(),
            authors: vec!["Jane Doe".into()],
            isbn: Some("9781111111111".into()),
            pubdate: Some("2021".into()),
            description: Some("A print book".into()),
            cover: Some(FilelessCover {
                mime: "image/gif".into(),
                bytes: GIF_1X1.to_vec(),
            }),
        },
    )
    .await
    .unwrap();

    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?1")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();

    // A uuid'd books row with the ISBN identifier, cover flagged, and no files.
    let has_cover: i64 = sqlx::query_scalar("SELECT has_cover FROM books WHERE id = ?1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(has_cover, 1);
    assert!(cover_path_for(&uuid, "gif").exists());

    let isbn: String = sqlx::query_scalar(
        "SELECT value FROM book_identifiers WHERE book_id = ?1 AND scheme = 'ISBN'",
    )
    .bind(book_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(isbn, "9781111111111");

    assert_eq!(
        count(
            &pool,
            &format!("SELECT COUNT(*) FROM book_files WHERE book_id = {book_id}")
        )
        .await,
        0
    );
    // Author linked.
    assert_eq!(
        count(
            &pool,
            &format!("SELECT COUNT(*) FROM books_authors_link WHERE book = {book_id}")
        )
        .await,
        1
    );
}

#[tokio::test]
async fn create_fileless_book_reuses_one_physical_scan_root() {
    let _covers = CoversTempDir::new("fileless_root");
    let pool = pool().await;

    for title in ["One", "Two"] {
        create_fileless_book(
            &pool,
            FilelessBook {
                title: title.into(),
                authors: vec![],
                isbn: None,
                pubdate: None,
                description: None,
                cover: None,
            },
        )
        .await
        .unwrap();
    }

    // Both fileless books share the single synthetic Physical scan root.
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM scan_roots WHERE path = 'physical://local'"
        )
        .await,
        1
    );
    let physical_books = count(
        &pool,
        "SELECT COUNT(*) FROM books b JOIN scan_roots s ON b.library_id = s.id
         WHERE s.path = 'physical://local'",
    )
    .await;
    assert_eq!(physical_books, 2);
}

#[tokio::test]
async fn create_fileless_book_links_multiple_authors_in_order_with_no_duplicates() {
    let _covers = CoversTempDir::new("fileless_multi_author");
    let pool = pool().await;

    let uuid = create_fileless_book(
        &pool,
        FilelessBook {
            title: "Collaborative Work".into(),
            // "Repeat Author" appears twice — the batched insert-or-ignore
            // must still link it only once, at its first position.
            authors: vec![
                "First Author".into(),
                "Repeat Author".into(),
                "Repeat Author".into(),
            ],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();

    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?1")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();

    let links: Vec<(String, i64)> = sqlx::query_as(
        "SELECT a.name, l.position FROM books_authors_link l
         JOIN authors a ON a.id = l.author
         WHERE l.book = ?1 ORDER BY l.position",
    )
    .bind(book_id)
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(
        links,
        vec![
            ("First Author".to_string(), 0),
            ("Repeat Author".to_string(), 1),
        ]
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM authors WHERE name = 'Repeat Author'"
        )
        .await,
        1
    );
}

/// `link_authors` chunks its batched inserts at 400 entries to stay under
/// SQLite's 999-bind-parameter cap; 850 authors forces three chunks (400 +
/// 400 + 50) and must still link every one, in order, with no drops at the
/// chunk boundary.
#[tokio::test]
async fn create_fileless_book_links_authors_above_the_sqlite_bind_cap() {
    let _covers = CoversTempDir::new("fileless_bind_cap");
    let pool = pool().await;
    let authors: Vec<String> = (0..850).map(|i| format!("Author {i:04}")).collect();

    let uuid = create_fileless_book(
        &pool,
        FilelessBook {
            title: "Massive Anthology".into(),
            authors: authors.clone(),
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();

    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?1")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();

    let linked: Vec<String> = sqlx::query_scalar(
        "SELECT a.name FROM books_authors_link l
         JOIN authors a ON a.id = l.author
         WHERE l.book = ?1 ORDER BY l.position",
    )
    .bind(book_id)
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(linked, authors);
}

#[tokio::test]
async fn create_fileless_book_returns_cover_error_when_cover_dir_path_is_a_file() {
    let covers = CoversTempDir::new("fileless_cover_error");
    // Occupy the covers-dir path with a regular file so `write_cover_file`'s
    // `create_dir_all` fails — the io-failure path `PhysicalError::Cover` wraps.
    std::fs::write(&covers.path, b"not a directory").unwrap();
    let pool = pool().await;

    let err = create_fileless_book(
        &pool,
        FilelessBook {
            title: "Broken Cover".into(),
            authors: vec![],
            isbn: None,
            pubdate: None,
            description: None,
            cover: Some(FilelessCover {
                mime: "image/gif".into(),
                bytes: GIF_1X1.to_vec(),
            }),
        },
    )
    .await
    .unwrap_err();

    assert!(matches!(err, PhysicalError::Cover(_)));
    // The whole transaction rolls back rather than leaving a `has_cover` row
    // with no file on disk.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 0);

    // `covers.path` is a regular file here, not a directory, so
    // `CoversTempDir`'s `Drop` (`remove_dir_all`) can't clean it up — remove
    // it explicitly to avoid leaking a stray file into the OS temp dir.
    let _ = std::fs::remove_file(&covers.path);
}

#[tokio::test]
async fn create_fileless_book_is_searchable_via_fts() {
    let _covers = CoversTempDir::new("fileless_fts");
    let pool = pool().await;

    let uuid = create_fileless_book(
        &pool,
        FilelessBook {
            title: "Unsearchable No More".into(),
            authors: vec!["Fts Author".into()],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();

    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?1")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();

    let hits: Vec<i64> = sqlx::query_scalar("SELECT rowid FROM books_fts WHERE books_fts MATCH ?1")
        .bind("Unsearchable")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(hits, vec![book_id]);
}

/// Minimal valid 1x1 GIF87a — enough for `image` to sniff and write `<uuid>.gif`.
const GIF_1X1: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x37, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xFF, 0xFF, 0xFF, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44,
    0x01, 0x00, 0x3B,
];

// --- note editing ----------------------------------------------------------

#[tokio::test]
async fn update_physical_copy_note_replaces_the_note() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let copy = add_physical_copy(&pool, "uuid-1", None, None, Some("1st ed"))
        .await
        .unwrap();

    let updated = update_physical_copy_note(&pool, copy.id, Some("signed 1st ed"))
        .await
        .unwrap();

    assert_eq!(updated.id, copy.id);
    assert_eq!(updated.note.as_deref(), Some("signed 1st ed"));
}

#[tokio::test]
async fn update_physical_copy_note_clears_the_note_when_blank() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let copy = add_physical_copy(&pool, "uuid-1", None, None, Some("1st ed"))
        .await
        .unwrap();

    // A whitespace-only note from an emptied input is a clear, not a value.
    let updated = update_physical_copy_note(&pool, copy.id, Some("   "))
        .await
        .unwrap();

    assert_eq!(updated.note, None);
}

#[tokio::test]
async fn update_physical_copy_note_returns_copy_not_found_for_unknown_id() {
    let pool = pool().await;
    let err = update_physical_copy_note(&pool, 9999, Some("x"))
        .await
        .unwrap_err();
    assert!(matches!(err, PhysicalError::CopyNotFound));
}

// --- wishlist lookup -------------------------------------------------------

#[tokio::test]
async fn get_wishlist_entry_returns_the_entry_when_wishlisted() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "reader").await;
    add_wishlist_entry(&pool, user, "uuid-1", WishlistSource::Detail)
        .await
        .unwrap();

    let entry = get_wishlist_entry(&pool, user, "uuid-1").await.unwrap();

    let entry = entry.expect("wishlisted book must return its entry");
    assert_eq!(entry.book_uuid, "uuid-1");
    assert_eq!(entry.source, WishlistSource::Detail);
}

#[tokio::test]
async fn get_wishlist_entry_returns_none_when_not_wishlisted() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let user = seed_user(&pool, "reader").await;

    assert!(get_wishlist_entry(&pool, user, "uuid-1")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn get_wishlist_entry_is_scoped_to_the_asking_user() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    add_wishlist_entry(&pool, alice, "uuid-1", WishlistSource::Detail)
        .await
        .unwrap();

    assert!(get_wishlist_entry(&pool, bob, "uuid-1")
        .await
        .unwrap()
        .is_none());
}

// --- fileless book removal -------------------------------------------------

#[tokio::test]
async fn delete_fileless_book_removes_the_book_and_its_soft_refs() {
    let _covers = CoversTempDir::new("delete_fileless");
    let pool = pool().await;
    let user = seed_user(&pool, "reader").await;
    let uuid = create_fileless_book(
        &pool,
        FilelessBook {
            title: "Paper Only".into(),
            authors: vec!["Ada Lovelace".into()],
            isbn: Some("9780000000001".into()),
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();
    add_physical_copy(&pool, &uuid, None, Some(user), None)
        .await
        .unwrap();
    add_wishlist_entry(&pool, user, &uuid, WishlistSource::Detail)
        .await
        .unwrap();
    // A uuid-keyed row *outside* the physical/wishlist/override trio, to prove
    // the delete sweeps the full canonical set rather than a hand-picked few.
    sqlx::query("INSERT INTO user_ratings (user_id, book_uuid, half_stars) VALUES (?1, ?2, 8)")
        .bind(user)
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();

    delete_fileless_book(&pool, &uuid).await.unwrap();

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 0);
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM physical_copies").await,
        0
    );
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM wishlist_entries").await,
        0
    );
    // The wider uuid-keyed sweep: the rating must go too.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM user_ratings").await, 0);
    // The FTS twin has no FK, so it only goes when the book is hard-deleted.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books_fts").await, 0);
    // Its now-bookless author goes with it (orphan taxonomy sweep).
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM authors").await, 0);
}

#[tokio::test]
async fn delete_fileless_book_refuses_a_book_that_still_has_files() {
    let pool = pool().await;
    seed_minimal_books(&pool, 1).await;

    let err = delete_fileless_book(&pool, "uuid-1").await.unwrap_err();

    assert!(matches!(err, PhysicalError::BookHasFiles));
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
}

#[tokio::test]
async fn delete_fileless_book_returns_book_not_found_for_unknown_uuid() {
    let pool = pool().await;
    let err = delete_fileless_book(&pool, "nope").await.unwrap_err();
    assert!(matches!(err, PhysicalError::BookNotFound));
}
