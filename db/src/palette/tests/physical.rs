//! Physical-only books under the synthetic `physical://local` root, which
//! is never a configured library path: the physical arm of the shared
//! visibility predicate is the only way they reach search, so each arm is
//! covered on its own, and a wishlist-only book stays hidden in every arm.

use sqlx::SqlitePool;

use super::super::*;
use crate::physical::{add_physical_copy, create_fileless_book, FilelessBook};
use crate::pool::init_db;
use crate::test_support::CoversTempDir;

// A physical-only book lives under the synthetic `physical://local` root,
// which is never a configured library path, so the physical arm of the
// shared visibility predicate is the only way it reaches search. Each arm
// gets its own test: they carried four independent copies of the scoping
// rule and can regress one at a time.
/// Mint a fileless book (synthetic `physical://local` root) with one author
/// and no copy — a wishlist-only entry, invisible to search.
async fn seed_fileless(pool: &SqlitePool, title: &str, author: &str) -> String {
    create_fileless_book(
        pool,
        FilelessBook {
            title: title.to_string(),
            authors: vec![author.to_string()],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap()
}

/// A fileless book with a checked-in print copy — the physical-only shape.
async fn seed_physical_only(pool: &SqlitePool, title: &str, author: &str) -> String {
    let uuid = seed_fileless(pool, title, author).await;
    add_physical_copy(pool, &uuid, None, None, None)
        .await
        .unwrap();
    uuid
}

/// Attach `uuid`'s book to a (created-on-demand) series.
async fn link_series(pool: &SqlitePool, uuid: &str, name: &str) {
    link_taxonomy(pool, uuid, name, "series", "books_series_link", "series").await;
}

/// Attach `uuid`'s book to a (created-on-demand) tag.
async fn link_tag(pool: &SqlitePool, uuid: &str, name: &str) {
    link_taxonomy(pool, uuid, name, "tags", "books_tags_link", "tag").await;
}

/// Resolve-or-insert a taxonomy row by name and link it to `uuid`'s book.
/// The two link tables differ only in their names, so one body serves both.
async fn link_taxonomy(
    pool: &SqlitePool,
    uuid: &str,
    name: &str,
    table: &str,
    link_table: &str,
    link_column: &str,
) {
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?1")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(&format!("INSERT OR IGNORE INTO {table} (name) VALUES (?1)"))
        .bind(name)
        .execute(pool)
        .await
        .unwrap();
    let id: i64 = sqlx::query_scalar(&format!("SELECT id FROM {table} WHERE name = ?1"))
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(&format!(
        "INSERT INTO {link_table} (book, {link_column}) VALUES (?1, ?2)"
    ))
    .bind(book_id)
    .bind(id)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn search_palette_finds_physical_only_book_when_it_has_a_copy() {
    let _covers = CoversTempDir::new("palette_physical_book");
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_physical_only(&pool, "Paper Only", "Ada Lovelace").await;

    let results = search_palette(&pool, "/lib", "paper").await.unwrap();

    assert_eq!(
        results.books.len(),
        1,
        "a checked-in print book must reach the palette, got {results:?}"
    );
    assert_eq!(results.books[0].title, "Paper Only");
    assert_eq!(results.books[0].author_display, "Ada Lovelace");
    assert!(
        results.books[0].formats.is_empty(),
        "a physical-only book carries no file formats"
    );
    assert_eq!(results.book_total, 1);

    // AC3: the palette and `/api/search` answer the same question.
    let full = crate::books::search_books_for_paths(&pool, &["/lib"], "paper")
        .await
        .unwrap();
    assert_eq!(full.len(), 1, "precondition: full search already found it");
}

#[tokio::test]
async fn search_palette_authors_arm_finds_an_author_known_only_from_a_physical_book() {
    let _covers = CoversTempDir::new("palette_physical_author");
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_physical_only(&pool, "Paper Only", "Ada Lovelace").await;

    let results = search_palette(&pool, "/lib", "lovelace").await.unwrap();

    assert_eq!(
        results.authors.len(),
        1,
        "physical-only book must surface its author, got {results:?}"
    );
    assert_eq!(results.authors[0].name, "Ada Lovelace");
    assert_eq!(
        results.authors[0].book_count, 1,
        "the effective-membership CTE must count the physical book too"
    );
    assert_eq!(
        results.authors[0].lead_book_title.as_deref(),
        Some("Paper Only")
    );
    assert_eq!(
        count_authors(&pool, "/lib", "%lovelace%").await.unwrap(),
        1,
        "the uncapped count must agree with the arm"
    );
}

#[tokio::test]
async fn search_palette_series_arm_finds_a_series_known_only_from_a_physical_book() {
    let _covers = CoversTempDir::new("palette_physical_series");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_physical_only(&pool, "Paper Saga #1", "Ada Lovelace").await;
    link_series(&pool, &uuid, "Paper Saga").await;

    let results = search_palette(&pool, "/lib", "paper saga").await.unwrap();

    assert_eq!(
        results.series.len(),
        1,
        "physical-only book must surface its series, got {results:?}"
    );
    assert_eq!(results.series[0].name, "Paper Saga");
    assert_eq!(results.series[0].book_count, 1);
    assert_eq!(
        results.series[0].author_display.as_deref(),
        Some("Ada Lovelace")
    );
    assert_eq!(
        results.series[0].lead_book_title.as_deref(),
        Some("Paper Saga #1")
    );
    assert_eq!(
        count_series(&pool, "/lib", "%paper saga%").await.unwrap(),
        1,
        "the uncapped count must agree with the arm"
    );
}

#[tokio::test]
async fn search_palette_tags_arm_finds_a_tag_known_only_from_a_physical_book() {
    let _covers = CoversTempDir::new("palette_physical_tag");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_physical_only(&pool, "Paper Only", "Ada Lovelace").await;
    link_tag(&pool, &uuid, "letterpress").await;

    let results = search_palette(&pool, "/lib", "letterpress").await.unwrap();

    assert_eq!(
        results.tags.len(),
        1,
        "physical-only book must surface its tag, got {results:?}"
    );
    assert_eq!(results.tags[0].name, "letterpress");
    assert_eq!(results.tags[0].book_count, 1);
    assert_eq!(
        count_tags(&pool, "/lib", "%letterpress%").await.unwrap(),
        1,
        "the uncapped count must agree with the arm"
    );
}

/// AC3, the other direction: a fileless book with no copy is a wishlist
/// entry. `/api/search` hides it, so every palette arm must too — the fix
/// admits books with a *copy*, not every book under the physical root.
#[tokio::test]
async fn search_palette_hides_a_wishlist_only_book_in_every_arm() {
    let _covers = CoversTempDir::new("palette_wishlist_only");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_fileless(&pool, "Papyrus Someday", "Papyrus Hopper").await;
    link_series(&pool, &uuid, "Papyrus Cycle").await;
    link_tag(&pool, &uuid, "papyrus").await;

    let results = search_palette(&pool, "/lib", "papyrus").await.unwrap();
    let full = crate::books::search_books_for_paths(&pool, &["/lib"], "papyrus")
        .await
        .unwrap();

    assert!(
        full.is_empty(),
        "precondition: full search hides a book with neither a file nor a copy"
    );
    assert!(results.books.is_empty(), "books arm: {results:?}");
    assert!(results.authors.is_empty(), "authors arm: {results:?}");
    assert!(results.series.is_empty(), "series arm: {results:?}");
    assert!(results.tags.is_empty(), "tags arm: {results:?}");
}
