use super::*;
use crate::books::list_books;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::*;

#[tokio::test]
async fn author_photo_roundtrips_manual_upload() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

    let bytes = b"\xFF\xD8\xFFfake-jpeg".to_vec();
    upsert_author_photo(
        &pool,
        ada_id,
        AuthorPhotoSource::Manual,
        None,
        Some("image/jpeg"),
        Some(&bytes),
    )
    .await
    .unwrap();

    let (mime, fetched) = get_author_photo(&pool, ada_id).await.unwrap().unwrap();
    assert_eq!(mime, "image/jpeg");
    assert_eq!(fetched, bytes);
}
#[tokio::test]
async fn author_photo_letter_marker_returns_none() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

    upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
        .await
        .unwrap();

    assert!(get_author_photo(&pool, ada_id).await.unwrap().is_none());

    let (src, _) = author_photo_status(&pool, ada_id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::Letter);
}
#[tokio::test]
async fn author_photo_status_none_when_unset() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;
    assert!(author_photo_status(&pool, ada_id).await.unwrap().is_none());
    assert!(get_author_photo(&pool, ada_id).await.unwrap().is_none());
}
#[tokio::test]
async fn author_photo_upsert_replaces_existing_row() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

    // Letter marker first, then a manual upload replaces it.
    upsert_author_photo(&pool, ada_id, AuthorPhotoSource::Letter, None, None, None)
        .await
        .unwrap();
    upsert_author_photo(
        &pool,
        ada_id,
        AuthorPhotoSource::Manual,
        None,
        Some("image/png"),
        Some(b"\x89PNG\r\n\x1a\nfake"),
    )
    .await
    .unwrap();

    let (src, _) = author_photo_status(&pool, ada_id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::Manual);
    let (mime, _) = get_author_photo(&pool, ada_id).await.unwrap().unwrap();
    assert_eq!(mime, "image/png");
}
#[tokio::test]
async fn author_photo_delete_clears_row() {
    let (pool, _guard) = seed_discovery_fixture().await;
    let ada_id = author_id_by_name(&pool, "Ada Lovelace").await;

    upsert_author_photo(
        &pool,
        ada_id,
        AuthorPhotoSource::Manual,
        None,
        Some("image/jpeg"),
        Some(b"\xFF\xD8\xFFfoo"),
    )
    .await
    .unwrap();
    delete_author_photo(&pool, ada_id).await.unwrap();

    assert!(author_photo_status(&pool, ada_id).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_author_removes_links_and_inserts_blocklist_row() {
    let _covers = CoversTempDir::new("delete_author_basic");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed("a.epub", Some("A"), &["Junk Author"], &[], None, None),
            indexed(
                "b.epub",
                Some("B"),
                &["Junk Author", "Real Author"],
                &[],
                None,
                None,
            ),
        ],
    )
    .await
    .unwrap();

    let junk_id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
        .bind("Junk Author")
        .fetch_one(&pool)
        .await
        .unwrap();

    let unlinked = delete_author(&pool, junk_id).await.unwrap();
    assert_eq!(unlinked, 2, "both books should report as un-linked");

    let junk_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE id = ?")
        .bind(junk_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(junk_count, 0, "authors row should be gone");

    let link_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM books_authors_link WHERE author = ?")
            .bind(junk_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        link_count, 0,
        "no link rows should remain for deleted author"
    );

    let blocklist_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM ignored_authors WHERE name = ?")
            .bind("Junk Author")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(blocklist_count, 1, "name must be added to ignored_authors");

    // Real Author on book B should still be linked.
    let books = list_books(&pool, "/lib").await.unwrap();
    let b = books
        .iter()
        .find(|x| x.title.as_deref() == Some("B"))
        .unwrap();
    let creators: Vec<&str> = b.creators.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(creators, vec!["Real Author"]);
}
#[tokio::test]
async fn delete_author_is_no_op_for_missing_id() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let unlinked = delete_author(&pool, 99_999).await.unwrap();
    assert_eq!(unlinked, 0);
    let blocklist_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM ignored_authors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        blocklist_count, 0,
        "missing id must not leak a stale blocklist row"
    );
}
#[tokio::test]
async fn delete_author_survives_reindex() {
    // Full durability check: delete the junk author, then run the
    // reindex pipeline against a fixture that *still* lists the junk
    // contributor in its OPF (simulated via replace_books with the
    // same input). The blocklist row inserted by delete_author must
    // keep resolve_or_insert_author from re-creating the author.
    let _covers = CoversTempDir::new("delete_author_reindex");
    let pool = init_db("sqlite::memory:").await.unwrap();

    let junk_name = "calibre (8.0.0) [https://calibre-ebook.com]";
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["Real Author", junk_name],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let junk_id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
        .bind(junk_name)
        .fetch_one(&pool)
        .await
        .unwrap();
    delete_author(&pool, junk_id).await.unwrap();

    // Simulated reindex: same OPF contents, run through the
    // indexing pipeline again. Without the blocklist guard this
    // would re-create the junk author and relink it.
    replace_books(
        &pool,
        "/lib",
        vec![indexed(
            "a.epub",
            Some("A"),
            &["Real Author", junk_name],
            &[],
            None,
            None,
        )],
    )
    .await
    .unwrap();

    let junk_after: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE name = ?")
        .bind(junk_name)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        junk_after, 0,
        "second reindex must not resurrect a deleted author"
    );

    let books = list_books(&pool, "/lib").await.unwrap();
    let creators: Vec<&str> = books[0].creators.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(creators, vec!["Real Author"]);
}
#[tokio::test]
async fn delete_author_rebuilds_fts_for_all_affected_books_in_batch() {
    // Verifies the batch FTS path: after deleting an author linked to
    // multiple books, each book's FTS row must exist and no longer contain
    // the deleted author's name.
    let _covers = CoversTempDir::new("delete_author_fts_batch");
    let pool = init_db("sqlite::memory:").await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![
            indexed(
                "a.epub",
                Some("Alpha"),
                &["Junk Author", "Keep Me"],
                &[],
                None,
                None,
            ),
            indexed("b.epub", Some("Beta"), &["Junk Author"], &[], None, None),
            indexed("c.epub", Some("Gamma"), &["Keep Me"], &[], None, None),
        ],
    )
    .await
    .unwrap();

    let junk_id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
        .bind("Junk Author")
        .fetch_one(&pool)
        .await
        .unwrap();

    let unlinked = delete_author(&pool, junk_id).await.unwrap();
    assert_eq!(unlinked, 2, "two books were linked to Junk Author");

    // All three books should still have an FTS row after the batch rebuild.
    let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        fts_count, 3,
        "every book must have an FTS row after the batch rebuild"
    );

    // The FTS rows for the two affected books must no longer contain the
    // deleted author's name.
    let junk_fts_count: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM books_fts WHERE authors MATCH 'junk'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(
        junk_fts_count, 0,
        "deleted author must not appear in any FTS row"
    );
}

#[tokio::test]
async fn get_author_photo_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = get_author_photo(&pool, 1).await.unwrap_err();
    assert!(matches!(err, AuthorPhotosDataError::Db(_)));
}
