//! The New bucket: `insert_book_row` (uuid minting, surname-first author
//! sort key), `sync_new`'s link rows, the `insert_chapters` and
//! `SyncError` failure paths, and the wishlist promotion
//! `try_attach_new_ebook` performs off the physical root.

use super::super::shared::insert_book_row;
use super::super::{sync_new, EntityAliasMaps, SyncError};
use super::{book_files_count, book_with_all_links, seed_scan_root};
use crate::ebook::IndexedBook;
use crate::pool::init_db;
use crate::test_support::{count_rows, indexed, CoversTempDir};

/// #2342: the Author axis must key every book on one format. A book with an
/// OPF `file_as` ("Weir, Andy") and one without (bare display name "Andy
/// Weir") by the same author used to store two incompatible sort keys — the
/// with-file_as book surname-first, the other given-first — and scatter to
/// opposite ends of the list. The write path now derives the surname-first
/// key from the display name when `file_as` is absent, so both land under one
/// key and sort adjacently.
#[tokio::test]
async fn insert_book_row_keys_author_sort_surname_first_with_and_without_file_as() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;

    let mut with_file_as = indexed(
        "martian.epub",
        Some("The Martian"),
        &["Andy Weir"],
        &[],
        None,
        None,
    );
    with_file_as.metadata.creators[0].file_as = Some("Weir, Andy".into());
    let without_file_as = indexed(
        "phm.epub",
        Some("Project Hail Mary"),
        &["Andy Weir"],
        &[],
        None,
        None,
    );

    let insert = |b: IndexedBook| {
        let pool = pool.clone();
        async move {
            let mut tx = pool.begin().await.unwrap();
            let id = insert_book_row(&mut tx, library_id, "/lib", &b)
                .await
                .unwrap()
                .book_id;
            tx.commit().await.unwrap();
            id
        }
    };
    let id_with = insert(with_file_as).await;
    let id_without = insert(without_file_as).await;

    let author_sort = |id: i64| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, String>("SELECT author_sort FROM books WHERE id = ?")
                .bind(id)
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    assert_eq!(author_sort(id_with).await, "Weir, Andy");
    assert_eq!(
        author_sort(id_without).await,
        "Weir, Andy",
        "a book without file_as must derive the same surname-first key"
    );
}

/// `sync_new` inserts a brand-new book: a canonical `books` row, its
/// `book_files` row, and every per-book link row.
#[tokio::test]
async fn sync_new_inserts_a_new_book_and_its_link_rows() {
    let _covers = CoversTempDir::new("sync_new_unit");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;

    let new_books = vec![book_with_all_links("fresh.epub", "Fresh")];
    let mut tx = pool.begin().await.unwrap();
    let covers = sync_new(
        &mut tx,
        library_id,
        "/lib",
        &new_books,
        &[],
        &EntityAliasMaps::default(),
        |_| {},
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert!(covers.is_empty(), "no cover was supplied");
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE scan_key = 'fresh.epub'")
        .fetch_one(&pool)
        .await
        .unwrap();
    let title: String = sqlx::query_scalar("SELECT title FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "Fresh");
    assert_eq!(
        book_files_count(&pool, book_id).await,
        1,
        "book_files inserted"
    );
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_authors_link WHERE book = {book_id}")
        )
        .await,
        1,
        "author link inserted"
    );
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_series_link WHERE book = {book_id}")
        )
        .await,
        1,
        "series link inserted"
    );
    // FTS row was written from the inserted rows.
    assert_eq!(
        count_rows(
            &pool,
            &format!("SELECT COUNT(*) FROM books_fts WHERE rowid = {book_id}")
        )
        .await,
        1,
        "FTS row inserted"
    );
}

/// A helper on the `IndexedBook` boundary: `insert_book_row` returns the
/// new id + a freshly minted uuid, and writes exactly one `book_files`
/// row alongside the `books` row.
#[tokio::test]
async fn insert_book_row_writes_books_and_book_files_and_mints_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let library_id = seed_scan_root(&pool).await;
    let b = indexed("solo.epub", Some("Solo"), &[], &[], None, None);

    let mut tx = pool.begin().await.unwrap();
    let inserted = insert_book_row(&mut tx, library_id, "/lib", &b)
        .await
        .unwrap();
    tx.commit().await.unwrap();

    assert!(!inserted.uuid.is_empty(), "a uuid is minted");
    let (uuid, scan_key): (String, String) =
        sqlx::query_as("SELECT uuid, scan_key FROM books WHERE id = ?")
            .bind(inserted.book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(uuid, inserted.uuid, "returned uuid matches the stored row");
    assert_eq!(scan_key, "solo.epub", "scan_key is the relative path");
    assert_eq!(
        book_files_count(&pool, inserted.book_id).await,
        1,
        "exactly one book_files row"
    );
    // The anchor row's scan_key must be set on insert, not left for a boot backfill.
    let file_scan_key: Option<String> =
        sqlx::query_scalar("SELECT scan_key FROM book_files WHERE book_id = ?")
            .bind(inserted.book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        file_scan_key.as_deref(),
        Some("solo.epub"),
        "book_files.scan_key is set on insert, matching books.scan_key"
    );
}

#[tokio::test]
async fn insert_chapters_propagates_db_error_when_table_missing() {
    // `SyncError` (crate-internal, shared by the ebook and audiobook sync
    // writers) has no direct pool-level entry point of its own — it's
    // produced deep inside the audiobook chapter writer. Dropping the
    // target table mid-transaction forces the same `sqlx::Error` passthrough
    // a closed pool would, without needing a second in-memory DB handle.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("DROP TABLE file_chapters")
        .execute(&mut *tx)
        .await
        .unwrap();
    let part = crate::audiobook::AudiobookPart {
        ordinal: 0,
        filename: "01.mp3".into(),
        size_bytes: 100,
        mtime_epoch: 0,
        duration_seconds: 10.0,
    };
    let err =
        crate::sync::audiobooks::insert_chapters(&mut tx, 1, &[], std::slice::from_ref(&part))
            .await
            .unwrap_err();
    assert!(matches!(err, super::super::SyncError::Db(_)));
}

/// The wishlist-then-acquired regression: a fileless check-in/wishlist book
/// lives under the `physical://local` pseudo-root, and the title+author
/// attach used to leave `books.library_id` there — fully indexed but
/// invisible to every path-scoped read. The attach must promote it into the
/// file's library.
#[tokio::test]
async fn try_attach_new_ebook_promotes_a_wishlist_target_off_the_physical_root() {
    let _covers = CoversTempDir::new("attach_promotes_wishlist");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let wishlist_uuid = crate::physical::create_fileless_book(
        &pool,
        crate::physical::FilelessBook {
            title: "Dream by the Shadows".into(),
            authors: vec!["Karlie Logan".into()],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();
    let lib_id = seed_scan_root(&pool).await;

    let b = indexed(
        "Logan/dream.epub",
        Some("Dream by the Shadows"),
        &["Karlie Logan"],
        &[],
        None,
        None,
    );
    let mut covers = Vec::new();
    let mut tx = pool.begin().await.unwrap();
    let attached = super::super::shared::try_attach_new_ebook(
        &mut tx,
        "/lib",
        &b,
        &std::collections::HashSet::new(),
        &EntityAliasMaps::default(),
        &mut covers,
    )
    .await
    .unwrap();
    tx.commit().await.unwrap();

    assert!(attached, "the file must attach to the wishlist book");
    let library_id: i64 = sqlx::query_scalar("SELECT library_id FROM books WHERE uuid = ?")
        .bind(&wishlist_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        library_id, lib_id,
        "attach must promote off the pseudo-root"
    );
    let listed = crate::books::list_books_for_paths(&pool, &["/lib"])
        .await
        .unwrap();
    assert!(
        listed
            .iter()
            .any(|x| x.unique_identifier.as_deref() == Some(wishlist_uuid.as_str())),
        "the promoted book must surface in the path-scoped listing"
    );
}

#[test]
fn sync_error_from_settings_error_returns_other_for_non_db_variants() {
    let validation = SyncError::from(crate::settings::SettingsError::Validation("bad key".into()));
    assert!(
        matches!(&validation, SyncError::Other(msg) if msg.contains("bad key")),
        "expected Other carrying the validation message, got {validation:?}"
    );

    let json_err = serde_json::from_str::<i32>("nope").unwrap_err();
    let json = SyncError::from(crate::settings::SettingsError::Json(json_err));
    assert!(
        matches!(json, SyncError::Other(_)),
        "expected Other, got {json:?}"
    );
}
