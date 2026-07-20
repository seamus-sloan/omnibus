//! Scan-resolution tests: each ladder rung + outcome variant (AC1), CloseMatch
//! stays a candidate (AC2), and the write composition (add physical-only,
//! wishlist by uuid / by meta).

use sqlx::SqlitePool;

use omnibus_shared::metadata_lookup::MetadataProvider;
use omnibus_shared::physical::WishlistSource;
use omnibus_shared::scan::ScanOutcome;

use super::*;
use crate::metadata_lookup::MetadataLookupConfig;
use crate::normalize::{normalize_author, normalize_title};
use crate::physical::{add_physical_copy, list_physical_copies, list_wishlist};
use serde_json::json;
use std::time::Duration;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

const ISBN: &str = "9780134685991";

async fn pool() -> SqlitePool {
    crate::pool::init_db("sqlite::memory:").await.unwrap()
}

/// Seed a normal (file-backed) library book with an author and optional ISBN.
async fn seed_book(pool: &SqlitePool, uuid: &str, title: &str, author: &str, isbn: Option<&str>) {
    sqlx::query("INSERT OR IGNORE INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(pool)
        .await
        .unwrap();
    let lib: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = '/lib'")
        .fetch_one(pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title, title_norm, author_norm, has_cover)
         VALUES (?1, ?2, '', ?3, ?4, ?5, 0) RETURNING id",
    )
    .bind(uuid)
    .bind(lib)
    .bind(title)
    .bind(normalize_title(title))
    .bind(normalize_author(author))
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO authors (name) VALUES (?1)")
        .bind(author)
        .execute(pool)
        .await
        .unwrap();
    let aid: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?1")
        .bind(author)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO books_authors_link (book, author, position) VALUES (?1, ?2, 0)")
        .bind(book_id)
        .bind(aid)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         VALUES (?1, 'EPUB', 'f', 1, 1)",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap();
    if let Some(isbn) = isbn {
        sqlx::query(
            "INSERT INTO book_identifiers (book_id, scheme, value) VALUES (?1, 'ISBN', ?2)",
        )
        .bind(book_id)
        .bind(isbn)
        .execute(pool)
        .await
        .unwrap();
    }
}

async fn seed_user(pool: &SqlitePool, username: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash) VALUES (?1, 'x') RETURNING id",
    )
    .bind(username)
    .fetch_one(pool)
    .await
    .unwrap()
}

/// A config whose provider base URLs point at the mock server.
fn config_for(server: &MockServer) -> MetadataLookupConfig {
    MetadataLookupConfig {
        openlibrary_base: server.uri(),
        googlebooks_base: server.uri(),
        timeout: Duration::from_secs(5),
    }
}

/// Mount Open Library to resolve `ISBN` to a book with the given title/author.
async fn mount_ol_hit(server: &MockServer, title: &str, author: &str) {
    Mock::given(method("GET"))
        .and(path("/api/books"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            format!("ISBN:{ISBN}"): {
                "title": title,
                "authors": [{ "name": author }],
            }
        })))
        .mount(server)
        .await;
}

/// Mount both providers to miss (unknown ISBN).
async fn mount_both_miss(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/books"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/books/v1/volumes"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "totalItems": 0 })))
        .mount(server)
        .await;
}

// ── ladder rungs (AC1) ─────────────────────────────────────────────

#[tokio::test]
async fn resolve_exact_isbn_returns_in_library_unowned() {
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", Some(ISBN)).await;
    let server = MockServer::start().await; // must not be hit

    let outcome = resolve_scan(&pool, ISBN, &config_for(&server))
        .await
        .unwrap();
    match outcome {
        ScanOutcome::InLibraryUnowned { book } => {
            assert_eq!(book.uuid, "u1");
            assert_eq!(book.authors, vec!["Joshua Bloch".to_string()]);
            assert!(!book.has_physical);
        }
        other => panic!("expected InLibraryUnowned, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_exact_isbn_returns_already_owned_when_physical_exists() {
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", Some(ISBN)).await;
    add_physical_copy(&pool, "u1", Some(ISBN), None, None)
        .await
        .unwrap();
    let server = MockServer::start().await;

    let outcome = resolve_scan(&pool, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        ScanOutcome::AlreadyOwned { book } if book.has_physical
    ));
}

#[tokio::test]
async fn resolve_close_match_via_online_then_norm() {
    let pool = pool().await;
    // Same title/author but NO matching ISBN identifier → exact rung misses.
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", None).await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "Effective Java", "Joshua Bloch").await;

    let outcome = resolve_scan(&pool, ISBN, &config_for(&server))
        .await
        .unwrap();
    match outcome {
        ScanOutcome::CloseMatch { book, scanned } => {
            assert_eq!(book.uuid, "u1");
            assert_eq!(scanned.source, MetadataProvider::OpenLibrary);
            assert_eq!(scanned.isbn13, ISBN);
        }
        other => panic!("expected CloseMatch, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_not_in_library_when_online_only() {
    let pool = pool().await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "Some Other Book", "Nobody Here").await;

    let outcome = resolve_scan(&pool, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(matches!(outcome, ScanOutcome::NotInLibrary { .. }));
}

#[tokio::test]
async fn resolve_unresolved_when_both_providers_miss() {
    let pool = pool().await;
    let server = MockServer::start().await;
    mount_both_miss(&server).await;

    let outcome = resolve_scan(&pool, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(matches!(outcome, ScanOutcome::Unresolved));
}

#[tokio::test]
async fn resolve_rejects_invalid_isbn() {
    let pool = pool().await;
    let server = MockServer::start().await;
    let err = resolve_scan(&pool, "12345", &config_for(&server))
        .await
        .unwrap_err();
    assert!(matches!(err, ScanError::Isbn(_)));
}

// ── write composition ──────────────────────────────────────────────

#[tokio::test]
async fn add_physical_only_creates_fileless_book_with_copy() {
    let pool = pool().await;
    let meta = omnibus_shared::metadata_lookup::ExternalBookMeta {
        isbn13: ISBN.into(),
        title: "Print Only".into(),
        authors: vec!["Jane Doe".into()],
        year: Some("2020".into()),
        pages: None,
        publisher: None,
        description: None,
        cover_url: None, // no cover → no fetch, no CoversTempDir needed
        source: MetadataProvider::OpenLibrary,
    };
    let uuid = add_physical_only(&pool, &meta, Some("first ed"), None)
        .await
        .unwrap();

    let copies = list_physical_copies(&pool, &uuid).await.unwrap();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].isbn.as_deref(), Some(ISBN));
    // The book resolves by its ISBN now (exact rung).
    let server = MockServer::start().await;
    let outcome = resolve_scan(&pool, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(matches!(outcome, ScanOutcome::AlreadyOwned { .. }));
}

#[tokio::test]
async fn wishlist_add_by_book_uuid() {
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", Some(ISBN)).await;
    let user = seed_user(&pool, "reader").await;

    let uuid = wishlist_add(&pool, user, Some("u1"), None, WishlistSource::Scan)
        .await
        .unwrap();
    assert_eq!(uuid, "u1");
    assert_eq!(list_wishlist(&pool, user).await.unwrap().len(), 1);
}

#[tokio::test]
async fn wishlist_add_by_meta_creates_fileless_book() {
    let pool = pool().await;
    let user = seed_user(&pool, "reader").await;
    let meta = omnibus_shared::metadata_lookup::ExternalBookMeta {
        isbn13: ISBN.into(),
        title: "Wishlisted".into(),
        authors: vec!["Jane Doe".into()],
        year: None,
        pages: None,
        publisher: None,
        description: None,
        cover_url: None,
        source: MetadataProvider::GoogleBooks,
    };

    let uuid = wishlist_add(&pool, user, None, Some(&meta), WishlistSource::Detail)
        .await
        .unwrap();
    let list = list_wishlist(&pool, user).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].book_uuid, uuid);
    // Fileless book exists but has no physical copy (pure wishlist entry).
    assert!(list_physical_copies(&pool, &uuid).await.unwrap().is_empty());
}

#[tokio::test]
async fn wishlist_add_errors_without_target() {
    let pool = pool().await;
    let user = seed_user(&pool, "reader").await;
    let err = wishlist_add(&pool, user, None, None, WishlistSource::Manual)
        .await
        .unwrap_err();
    assert!(matches!(err, ScanError::MissingWishlistTarget));
}
