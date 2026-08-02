//! Unit tests for the `taxonomy` helpers. One `resolve_or_insert_simple!`
//! macro generates the three resolvers, so all three contracts (insert,
//! idempotency, case-insensitivity) are covered on `series` and spot-checked
//! on `publishers` / `languages` to catch a bad expansion.
//! `delete_orphan_taxonomy` gets its own drop/keep pair;
//! `delete_orphan_tags` adds the override-aware keep/drop cases.

use omnibus_shared::MetadataOverrides;

use super::*;
use crate::books::list_books;
use crate::metadata_overrides::upsert_metadata_overrides;
use crate::pool::init_db;
use crate::sync::replace_books;
use crate::test_support::{indexed, CoversTempDir};

#[tokio::test]
async fn resolve_or_insert_series_inserts_and_returns_id() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let id = resolve_or_insert_series(&mut tx, "Science Fiction")
        .await
        .unwrap();
    assert!(id > 0, "expected a positive row id, got {id}");
    tx.commit().await.unwrap();

    let stored: String = sqlx::query_scalar("SELECT name FROM series WHERE id = ?")
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, "Science Fiction");
}

#[tokio::test]
async fn resolve_or_insert_series_is_idempotent_for_same_name() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let id1 = resolve_or_insert_series(&mut tx, "Fantasy").await.unwrap();
    let id2 = resolve_or_insert_series(&mut tx, "Fantasy").await.unwrap();
    assert_eq!(id1, id2, "second insert must reuse the existing row id");
    tx.commit().await.unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series WHERE name = ?")
        .bind("Fantasy")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "expected exactly one series row, got {count}");
}

#[tokio::test]
async fn resolve_or_insert_series_collapses_case_variants_to_one_row() {
    // The `series.name` column is declared `COLLATE NOCASE` (see
    // migration 0002), so `INSERT OR IGNORE` treats case variants as
    // duplicates and the follow-up `SELECT … WHERE name = ?` resolves
    // back to the row stored under the first-seen case.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let id_mixed = resolve_or_insert_series(&mut tx, "Fantasy").await.unwrap();
    let id_lower = resolve_or_insert_series(&mut tx, "fantasy").await.unwrap();
    let id_upper = resolve_or_insert_series(&mut tx, "FANTASY").await.unwrap();
    assert_eq!(id_mixed, id_lower);
    assert_eq!(id_mixed, id_upper);
    tx.commit().await.unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 1, "case variants must not insert extra rows");

    // First-seen case is preserved — `INSERT OR IGNORE` is a no-op on the
    // case-different follow-ups, so the stored value is the original.
    let stored: String = sqlx::query_scalar("SELECT name FROM series WHERE id = ?")
        .bind(id_mixed)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, "Fantasy");
}

#[tokio::test]
async fn resolve_or_insert_publisher_inserts_and_is_idempotent() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let id1 = resolve_or_insert_publisher(&mut tx, "Penguin Random House")
        .await
        .unwrap();
    let id2 = resolve_or_insert_publisher(&mut tx, "Penguin Random House")
        .await
        .unwrap();
    assert!(id1 > 0);
    assert_eq!(id1, id2);
    tx.commit().await.unwrap();

    let stored: String = sqlx::query_scalar("SELECT name FROM publishers WHERE id = ?")
        .bind(id1)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(stored, "Penguin Random House");
}

#[tokio::test]
async fn resolve_or_insert_language_keys_off_the_code_column() {
    // `languages.code` is the keyed column (not `name`), so this test
    // also catches the macro accidentally targeting the wrong column.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    let id_en = resolve_or_insert_language(&mut tx, "en").await.unwrap();
    let id_en_upper = resolve_or_insert_language(&mut tx, "EN").await.unwrap();
    let id_fr = resolve_or_insert_language(&mut tx, "fr").await.unwrap();
    assert!(id_en > 0);
    assert_eq!(id_en, id_en_upper, "code column is COLLATE NOCASE");
    assert_ne!(id_en, id_fr, "distinct codes must get distinct ids");
    tx.commit().await.unwrap();

    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM languages")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(count, 2);
}

#[tokio::test]
async fn delete_orphan_taxonomy_drops_childless_rows_across_all_five_tables() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut tx = pool.begin().await.unwrap();
    sqlx::query("INSERT INTO authors (name) VALUES ('Nobody')")
        .execute(&mut *tx)
        .await
        .unwrap();
    resolve_or_insert_series(&mut tx, "Abandoned Saga")
        .await
        .unwrap();
    resolve_or_insert_tag(&mut tx, "unused").await.unwrap();
    resolve_or_insert_publisher(&mut tx, "Defunct Press")
        .await
        .unwrap();
    resolve_or_insert_language(&mut tx, "xx").await.unwrap();

    delete_orphan_taxonomy(&mut tx).await.unwrap();
    tx.commit().await.unwrap();

    for table in ["authors", "series", "tags", "publishers", "languages"] {
        let n: i64 = sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {table}"))
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 0, "{table} should have no childless rows left");
    }
}

#[tokio::test]
async fn delete_orphan_taxonomy_keeps_rows_that_still_have_a_linked_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // The seeder links one book to author "Prolific" and series "Mega";
    // "Dropped" is added with no link row at all.
    let (_author_id, series_id) =
        crate::test_support::seed_books_for_one_author_and_series(&pool, 1).await;
    let mut tx = pool.begin().await.unwrap();
    resolve_or_insert_series(&mut tx, "Dropped").await.unwrap();

    delete_orphan_taxonomy(&mut tx).await.unwrap();
    tx.commit().await.unwrap();

    let names: Vec<String> = sqlx::query_scalar("SELECT name FROM series ORDER BY name")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(names, vec!["Mega".to_string()], "linked series retained");
    let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series WHERE id = ?")
        .bind(series_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kept, 1);
    let authors: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(authors, 1, "linked author retained");
}

#[tokio::test]
async fn delete_orphan_tags_keeps_a_tag_named_only_by_a_live_books_override() {
    let _covers = CoversTempDir::new("orphan_tags_override_keep");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
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
    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();
    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            subjects: Some(vec!["Keeper".into()]),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    delete_orphan_tags(&mut tx).await.unwrap();
    tx.commit().await.unwrap();

    let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags WHERE name = 'Keeper'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(kept, 1, "an override membership must protect the tag row");
}

#[tokio::test]
async fn delete_orphan_tags_matches_override_subjects_case_insensitively() {
    let _covers = CoversTempDir::new("orphan_tags_case_fold");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user_id = crate::auth::create_user(&pool, "admin", "securepassword1")
        .await
        .unwrap()
        .id;
    // A linkless canonical-cased row, protected only by a lower-cased
    // override subject on a live book — the NOCASE tags unique means the
    // override materializes no second row, so the sweep's membership match
    // must fold case or the row is wrongly reaped.
    let mut tx = pool.begin().await.unwrap();
    resolve_or_insert_tag(&mut tx, "Fiction").await.unwrap();
    tx.commit().await.unwrap();
    replace_books(
        &pool,
        "/lib",
        vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
    )
    .await
    .unwrap();
    let books = list_books(&pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();
    upsert_metadata_overrides(
        &pool,
        &uuid,
        &MetadataOverrides {
            subjects: Some(vec!["fiction".into()]),
            ..Default::default()
        },
        false,
        user_id,
    )
    .await
    .unwrap();

    let mut tx = pool.begin().await.unwrap();
    delete_orphan_tags(&mut tx).await.unwrap();
    tx.commit().await.unwrap();

    let kept: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags WHERE name = 'Fiction'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        kept, 1,
        "a case-variant override membership must protect the row"
    );
}

#[tokio::test]
async fn delete_orphan_tags_ignores_override_rows_whose_book_no_longer_exists() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // An override row keyed to a uuid with no live `books` row — e.g. left
    // behind by an out-of-band delete — must not keep its tags alive. The
    // membership check joins to `books`, mirroring `get_tag_cloud`.
    sqlx::query(
        r#"INSERT INTO metadata_overrides (book_uuid, overrides)
           VALUES ('no-such-book', '{"subjects":["Ghosted"]}')"#,
    )
    .execute(&pool)
    .await
    .unwrap();
    let mut tx = pool.begin().await.unwrap();
    resolve_or_insert_tag(&mut tx, "Ghosted").await.unwrap();
    delete_orphan_tags(&mut tx).await.unwrap();
    tx.commit().await.unwrap();

    let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tags WHERE name = 'Ghosted'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(n, 0, "a dead book's override must not protect a tag row");
}
