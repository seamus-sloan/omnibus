use super::*;
use crate::books::list_books;
use crate::metadata_overrides::{upsert_metadata_overrides, write_override_cover};
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};
use omnibus_shared::MetadataOverrides;

#[tokio::test]
async fn get_last_modified_epoch_falls_back_to_now_when_column_null() {
    // `books.last_modified` is nullable after 0038's in-place conversion; a bare
    // `i64` decode of a NULL would error and 500 `/api/thumbs/*`. The COALESCE
    // keeps it serving (and regenerates the stale thumb).
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO scan_roots (id, path, display_name) VALUES (1, '/lib', 'Lib')")
        .execute(&pool)
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, last_modified) \
         VALUES ('bk', 'b', 1, '/lib/bk', 'Book', NULL) RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let got = get_last_modified_epoch(&pool, id).await.unwrap();
    assert!(matches!(got, Some(e) if e > 1_700_000_000), "got {got:?}");
    // A missing book still yields None rather than a fabricated epoch.
    assert_eq!(get_last_modified_epoch(&pool, 99_999).await.unwrap(), None);
}

#[tokio::test]
async fn cover_returns_none_when_file_missing() {
    let _covers = CoversTempDir::new("missing");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["A"],
            &[],
            None,
            Some(("image/jpeg", b"BYTES")),
        )],
    )
    .await
    .unwrap();
    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(books[0].id)
        .fetch_one(&pool)
        .await
        .unwrap();
    // Remove the file out from under the DB — get_cover should report
    // None, not error.
    let _ = std::fs::remove_file(cover_path_for(&uuid, "jpg"));
    assert!(get_cover(&pool, books[0].id).await.unwrap().is_none());
}
/// When a `metadata_overrides` row sets `has_cover_override = 1` and an
/// `override-<uuid>.<ext>` file exists on disk, `get_cover` returns the
/// override bytes — not the scanned cover. Single-query form must
/// preserve this precedence.
#[tokio::test]
async fn cover_returns_override_when_flag_set() {
    let _covers = CoversTempDir::new("override_set");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["A"],
            &[],
            None,
            Some(("image/jpeg", b"ORIGINAL")),
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();

    // Mark cover-override + drop the override file on disk.
    write_override_cover(&uuid, "image/png", b"OVERRIDE").unwrap();
    upsert_metadata_overrides(&pool, &uuid, &MetadataOverrides::default(), true, user_id)
        .await
        .unwrap();

    let cover = get_cover(&pool, books[0].id).await.unwrap();
    assert_eq!(cover, Some(("image/png".into(), b"OVERRIDE".to_vec())));
}
/// With no `metadata_overrides` row, `get_cover` falls through to the
/// scanned `<uuid>.<ext>` cover. The LEFT JOIN must not filter the book
/// out when no override row exists.
#[tokio::test]
async fn cover_returns_original_when_no_override_row() {
    let _covers = CoversTempDir::new("override_absent");
    let pool = init_db("sqlite::memory:").await.unwrap();

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["A"],
            &[],
            None,
            Some(("image/jpeg", b"ORIGINAL")),
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let cover = get_cover(&pool, books[0].id).await.unwrap();
    assert_eq!(cover, Some(("image/jpeg".into(), b"ORIGINAL".to_vec())));
}
/// A `metadata_overrides` row with `has_cover_override = 0` (text-only
/// edits, no cover swap) must resolve to the scanned cover, not the
/// override path.
#[tokio::test]
async fn cover_returns_original_when_override_flag_unset() {
    let _covers = CoversTempDir::new("override_flag_off");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;

    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["A"],
            &[],
            None,
            Some(("image/jpeg", b"ORIGINAL")),
        )],
    )
    .await
    .unwrap();

    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();

    // Override row exists with text edits but no cover swap.
    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            title: Some("Edited".into()),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let cover = get_cover(&pool, books[0].id).await.unwrap();
    assert_eq!(cover, Some(("image/jpeg".into(), b"ORIGINAL".to_vec())));
}
/// `get_cover` for a non-existent book id returns `Ok(None)` (not an
/// error). The LEFT JOIN must not change this contract.
#[tokio::test]
async fn cover_returns_none_for_missing_book_id() {
    let _covers = CoversTempDir::new("missing_book");
    let pool = init_db("sqlite::memory:").await.unwrap();
    assert!(get_cover(&pool, 999_999).await.unwrap().is_none());
}
