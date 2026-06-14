use super::*;
use crate::pool::init_db;

#[test]
fn normalize_title_casefolds_and_collapses_punctuation() {
    assert_eq!(normalize_title("Dracula"), Some("dracula".into()));
    assert_eq!(normalize_title("  DRACULA!  "), Some("dracula".into()));
    assert_eq!(
        normalize_title("The Hitch-Hiker's Guide"),
        Some("the hitch hiker s guide".into())
    );
    assert_eq!(
        normalize_title("Dune:  Messiah"),
        Some("dune messiah".into())
    );
}

#[test]
fn normalize_title_folds_diacritics() {
    assert_eq!(normalize_title("Çítåtelka"), Some("citatelka".into()));
    assert_eq!(normalize_title("Mémoires"), Some("memoires".into()));
}

#[test]
fn normalize_title_returns_none_when_nothing_survives() {
    assert_eq!(normalize_title(""), None);
    assert_eq!(normalize_title("  --- "), None);
}

#[test]
fn normalize_title_keeps_distinct_titles_distinct() {
    // The whole point of exact matching: no substring/fuzzy collapse.
    assert_ne!(normalize_title("Dune"), normalize_title("Dune Messiah"));
}

#[test]
fn normalize_author_swaps_single_comma_last_first() {
    assert_eq!(normalize_author("Stoker, Bram"), Some("bram stoker".into()));
    assert_eq!(normalize_author("Bram Stoker"), Some("bram stoker".into()));
    // Two commas: not a Last, First form — fold as-is.
    assert_eq!(
        normalize_author("Smith, John, Jr."),
        Some("smith john jr".into())
    );
}

#[tokio::test]
async fn backfill_norm_columns_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = backfill_norm_columns(&pool).await.unwrap_err();
    assert!(matches!(err, NormalizeError::Db(_)));
}

#[tokio::test]
async fn backfill_norm_columns_fills_only_null_rows_and_is_idempotent() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    sqlx::query("INSERT INTO libraries (path, display_name) VALUES ('/lib', 'lib')")
        .execute(&pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title) \
         VALUES ('u1', 1, '', 'Drácula!') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO authors (name, sort) VALUES ('Stoker, Bram', 'Stoker, Bram')")
        .execute(&pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO books_authors_link (book, author, position) VALUES (?, 1, 0)")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();

    backfill_norm_columns(&pool).await.unwrap();

    let (title_norm, author_norm): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT title_norm, author_norm FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(title_norm.as_deref(), Some("dracula"));
    assert_eq!(author_norm.as_deref(), Some("bram stoker"));

    // A second run must not touch already-backfilled rows.
    sqlx::query("UPDATE books SET author_norm = 'sentinel' WHERE id = ?")
        .bind(book_id)
        .execute(&pool)
        .await
        .unwrap();
    backfill_norm_columns(&pool).await.unwrap();
    let (_, author_norm): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT title_norm, author_norm FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(author_norm.as_deref(), Some("sentinel"));
}
