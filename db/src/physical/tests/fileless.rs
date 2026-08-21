//! Fileless (physical-only) book creation: the uuid'd `books` row with its
//! identifier, cover, and author links, the shared synthetic scan root, FTS
//! visibility, and the cover-write failure path.

use crate::covers::cover_path_for;
use crate::test_support::CoversTempDir;

use super::super::*;
use super::{count, pool};

/// Minimal valid 1x1 GIF87a — enough for `image` to sniff and write `<uuid>.gif`.
const GIF_1X1: &[u8] = &[
    0x47, 0x49, 0x46, 0x38, 0x37, 0x61, 0x01, 0x00, 0x01, 0x00, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00,
    0xFF, 0xFF, 0xFF, 0x2C, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00, 0x02, 0x02, 0x44,
    0x01, 0x00, 0x3B,
];

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
