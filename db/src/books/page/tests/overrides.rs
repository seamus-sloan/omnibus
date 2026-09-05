//! Sorting on the displayed metadata rather than the scanned file: the
//! title, author (`file_as` first) and series axes key on the override,
//! an override its scan root ranks below the scan is ignored, and the
//! title axis stays case-insensitive across overrides.

use omnibus_shared::{SortDir, SortKey, ViewFilters};
use sqlx::SqlitePool;

use super::super::*;
use super::{ids, insert_book, insert_lib, titles};
use crate::pool::init_db;

// Sorting keys on the *displayed* metadata, not the scanned file (#2258).
/// Insert a book carrying an `author_sort`, plus its backing file row.
async fn insert_authored_book(
    pool: &SqlitePool,
    lib_id: i64,
    title: &str,
    author_sort: &str,
) -> i64 {
    let id = insert_book(pool, lib_id, title, Some(title), None, None).await;
    sqlx::query("UPDATE books SET author_sort = ? WHERE id = ?")
        .bind(author_sort)
        .bind(id)
        .execute(pool)
        .await
        .unwrap();
    id
}

/// Write a raw `metadata_overrides` row for `book_id`.
async fn set_overrides_json(pool: &SqlitePool, book_id: i64, json: &str) {
    let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO metadata_overrides (book_uuid, overrides) VALUES (?, ?)")
        .bind(uuid)
        .bind(json)
        .execute(pool)
        .await
        .unwrap();
}

async fn page_by(pool: &SqlitePool, sort: SortKey) -> BookPage {
    list_books_page(
        pool,
        &["/lib"],
        sort,
        SortDir::Asc,
        &ViewFilters::default(),
        &[],
        None,
        50,
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn list_books_page_orders_title_and_author_on_the_override_not_the_scan() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    // Corrected: the scan filed it last on both axes, the override files it
    // first on both — so a stale key is visible as a reversed page.
    let corrected = insert_authored_book(&pool, lib, "Zulu", "Zulu Author").await;
    let scanned = insert_authored_book(&pool, lib, "Mango", "Mango Author").await;
    set_overrides_json(
        &pool,
        corrected,
        r#"{"title":"Aardvark","creators":[{"name":"Alpha Wells"}]}"#,
    )
    .await;

    let by_title = page_by(&pool, SortKey::Title).await;
    assert_eq!(ids(&by_title), vec![corrected, scanned]);
    assert_eq!(
        titles(&by_title),
        vec!["Aardvark".to_string(), "Mango".to_string()],
        "the page sorts by the same title it displays"
    );

    assert_eq!(
        ids(&page_by(&pool, SortKey::Author).await),
        vec![corrected, scanned]
    );
}

#[tokio::test]
async fn list_books_page_prefers_an_override_creators_file_as_over_its_name() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    let filed = insert_authored_book(&pool, lib, "One", "Aaa Scanned").await;
    let plain = insert_authored_book(&pool, lib, "Two", "Bbb Scanned").await;
    // `file_as` is how the scanner fills `author_sort`, so an override that
    // carries one must win over its display name.
    set_overrides_json(
        &pool,
        filed,
        r#"{"creators":[{"name":"Amy Zeta","file_as":"Zeta, Amy"}]}"#,
    )
    .await;
    set_overrides_json(&pool, plain, r#"{"creators":[{"name":"Martha Wells"}]}"#).await;

    assert_eq!(
        ids(&page_by(&pool, SortKey::Author).await),
        vec![plain, filed]
    );
}

#[tokio::test]
async fn list_books_page_orders_series_on_the_override_name() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    // Rehomed into a series that files ahead of the one the scan found.
    let rehomed = insert_book(&pool, lib, "B", Some("B"), Some("Zed Cycle"), Some(1.0)).await;
    let native = insert_book(&pool, lib, "A", Some("A"), Some("Mango Cycle"), Some(1.0)).await;
    set_overrides_json(&pool, rehomed, r#"{"series":"Alpha Cycle"}"#).await;

    assert_eq!(
        ids(&page_by(&pool, SortKey::Series).await),
        vec![rehomed, native]
    );
}

#[tokio::test]
async fn list_books_page_orders_series_on_the_override_in_series_index() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    let renumbered = insert_book(&pool, lib, "A", Some("A"), Some("Zed Cycle"), Some(1.0)).await;
    let sibling = insert_book(&pool, lib, "B", Some("B"), Some("Zed Cycle"), Some(2.0)).await;
    // Renumbered behind its sibling; the index is stored as text, like the
    // wire type the editor submits.
    set_overrides_json(&pool, renumbered, r#"{"series_index":"9"}"#).await;

    assert_eq!(
        ids(&page_by(&pool, SortKey::Series).await),
        vec![sibling, renumbered]
    );
}

#[tokio::test]
async fn list_books_page_ignores_an_override_its_scan_root_ranks_below_the_scan() {
    use omnibus_shared::MetadataSource::*;

    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    let overridden = insert_authored_book(&pool, lib, "Zulu", "Zulu Author").await;
    let scanned = insert_authored_book(&pool, lib, "Mango", "Mango Author").await;
    set_overrides_json(&pool, overridden, r#"{"title":"Aardvark"}"#).await;

    // The scan root ranks the scanned tags above the override layer, so the
    // page shows — and must therefore sort by — the scanned title.
    crate::settings::set_metadata_precedence(
        &pool,
        "/lib",
        &[
            FolderStructure,
            OmnibusOverrides,
            EmbeddedTags,
            OpfSidecar,
            ProviderMatch,
        ],
    )
    .await
    .unwrap();

    let page = page_by(&pool, SortKey::Title).await;
    assert_eq!(ids(&page), vec![scanned, overridden]);
    assert_eq!(titles(&page), vec!["Mango".to_string(), "Zulu".to_string()]);
}

#[tokio::test]
async fn list_books_page_keeps_the_title_axis_case_insensitive_across_overrides() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = insert_lib(&pool, "/lib").await;
    // A COALESCE expression carries no implicit collation, so without the
    // restated NOCASE the lowercase title would sort after every capital.
    let lower = insert_book(&pool, lib, "apple", Some("apple"), None, None).await;
    let upper = insert_book(&pool, lib, "Banana", Some("Banana"), None, None).await;

    assert_eq!(
        ids(&page_by(&pool, SortKey::Title).await),
        vec![lower, upper]
    );
}
