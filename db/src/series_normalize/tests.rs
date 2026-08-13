//! Tests for `backfill_embedded_series_index`.

use sqlx::SqlitePool;

use super::*;
use crate::pool::init_db;

async fn seed_scan_root(pool: &SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

/// Seed a fragmented series row + one linked book, standing in for data
/// written before #1912: the series name still embeds its index and
/// `explicit_index` is what (if anything) the scanner had stored in
/// `books.series_index` directly.
async fn seed_fragmented_book(
    pool: &SqlitePool,
    lib_id: i64,
    uuid: &str,
    series_name: &str,
    explicit_index: Option<f64>,
) -> i64 {
    let series_id: i64 = sqlx::query_scalar("INSERT INTO series (name) VALUES (?1) RETURNING id")
        .bind(series_name)
        .fetch_one(pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title, series_index)
         VALUES (?1, ?2, '', 'Book', ?3) RETURNING id",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(explicit_index)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO books_series_link (book, series) VALUES (?1, ?2)")
        .bind(book_id)
        .bind(series_id)
        .execute(pool)
        .await
        .unwrap();
    book_id
}

#[tokio::test]
async fn backfill_embedded_series_index_merges_hash_indexed_rows_into_one_series() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = seed_scan_root(&pool).await;
    let b1 = seed_fragmented_book(&pool, lib_id, "u1", "Gods and Monsters #1", None).await;
    let b2 = seed_fragmented_book(&pool, lib_id, "u2", "Gods and Monsters #2", None).await;
    let b3 = seed_fragmented_book(&pool, lib_id, "u3", "Gods and Monsters #3", None).await;

    backfill_embedded_series_index(&pool).await.unwrap();

    let series: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM series")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(
        series.len(),
        1,
        "fragmented rows merge into one: {series:?}"
    );
    assert_eq!(series[0].1, "Gods and Monsters");

    for (book_id, want) in [(b1, 1.0), (b2, 2.0), (b3, 3.0)] {
        let idx: f64 = sqlx::query_scalar("SELECT series_index FROM books WHERE id = ?1")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(idx, want);
        let linked: i64 =
            sqlx::query_scalar("SELECT series FROM books_series_link WHERE book = ?1")
                .bind(book_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(linked, series[0].0, "book relinks onto the merged row");
    }
}

#[tokio::test]
async fn backfill_embedded_series_index_only_fills_a_missing_series_index() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = seed_scan_root(&pool).await;
    // AC2: an explicit index already on the row must survive untouched, even
    // though the embedded name disagrees with it.
    let book_id = seed_fragmented_book(&pool, lib_id, "u1", "Mistborn #2", Some(99.0)).await;

    backfill_embedded_series_index(&pool).await.unwrap();

    let idx: f64 = sqlx::query_scalar("SELECT series_index FROM books WHERE id = ?1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        idx, 99.0,
        "an explicit index is never overwritten by the embedded one"
    );
}

#[tokio::test]
async fn backfill_embedded_series_index_removes_the_orphaned_fragment_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = seed_scan_root(&pool).await;
    seed_fragmented_book(&pool, lib_id, "u1", "Bride of the Shadow King #2", None).await;

    backfill_embedded_series_index(&pool).await.unwrap();

    // AC3: no leftover fragment series row to show up on /series.
    let leftover: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM series WHERE name LIKE '%#%'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(leftover, 0);
}

#[tokio::test]
async fn backfill_embedded_series_index_leaves_a_plain_series_name_untouched() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = seed_scan_root(&pool).await;
    let book_id = seed_fragmented_book(&pool, lib_id, "u1", "Foundation", None).await;

    backfill_embedded_series_index(&pool).await.unwrap();

    let name: String = sqlx::query_scalar("SELECT name FROM series")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(name, "Foundation");
    let idx: Option<f64> = sqlx::query_scalar("SELECT series_index FROM books WHERE id = ?1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(
        idx, None,
        "no embedded index to fill in, so none is invented"
    );
}

#[tokio::test]
async fn backfill_embedded_series_index_handles_the_comma_book_variant_and_is_idempotent() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib_id = seed_scan_root(&pool).await;
    let book_id = seed_fragmented_book(&pool, lib_id, "u1", "Mistborn, Book 2", None).await;

    backfill_embedded_series_index(&pool).await.unwrap();
    // A second pass must be a no-op — the merged row no longer matches the
    // embedded-index pattern.
    backfill_embedded_series_index(&pool).await.unwrap();

    let series: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM series")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(series.len(), 1);
    assert_eq!(series[0].1, "Mistborn");
    let idx: f64 = sqlx::query_scalar("SELECT series_index FROM books WHERE id = ?1")
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(idx, 2.0);
}

#[tokio::test]
async fn backfill_embedded_series_index_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = backfill_embedded_series_index(&pool).await.unwrap_err();
    assert!(matches!(err, SeriesNormalizeError::Db(_)));
}
