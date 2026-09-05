//! Scan-resolution tests, split by rung and outcome into the sibling
//! modules below; the seeded-book, user, config and wiremock provider
//! fixtures they share live here.

mod candidates;
mod close_match;
mod exact_rung;
mod outcomes;
mod writes;

use std::time::Duration;

use omnibus_shared::metadata_lookup::MetadataProvider;
use omnibus_shared::scan::ScanOutcome;
use omnibus_shared::{Contributor, MetadataOverrides};
use serde_json::json;
use sqlx::SqlitePool;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::metadata_lookup::{MetadataLookupConfig, ProviderKeys};
use crate::normalize::{normalize_author, normalize_title};

const ISBN: &str = "9780134685991";

/// A resolving user for tests that don't exercise the wishlist branch. The
/// wishlist lookup is scoped by this id; with no entry seeded it resolves to
/// `None`, so the id needn't reference a real user for those cases.
const USER_ID: i64 = 1;

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
    // OR IGNORE so a second book by the same author (norm-ambiguity /
    // exact-vs-tolerant tests) doesn't trip the UNIQUE(name) constraint.
    sqlx::query("INSERT OR IGNORE INTO authors (name) VALUES (?1)")
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

/// Apply a title/author override to a seeded book via the real save path
/// (which populates `metadata_overrides.(title_norm, author_norm)`).
async fn override_title_author(
    pool: &SqlitePool,
    uuid: &str,
    user_id: i64,
    title: Option<&str>,
    author: Option<&str>,
) {
    let overrides = MetadataOverrides {
        title: title.map(str::to_string),
        creators: author.map(|a| {
            vec![Contributor {
                name: a.to_string(),
                role: None,
                file_as: None,
                id: None,
            }]
        }),
        ..Default::default()
    };
    crate::merge_metadata_overrides(pool, uuid, &overrides, user_id)
        .await
        .unwrap();
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
        // Keyless on purpose: the mock never checks it, and reading the real
        // env here would make the suite depend on the developer's `.env`.
        hardcover_base: server.uri(),
        keys: ProviderKeys::default(),
        // An isolated tracker: a cooldown must never leak between tests.
        throttle: crate::metadata_lookup::ThrottleTracker::fresh(),
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

/// Every uuid a `CloseMatch` offers, head and tail flattened the way both
/// clients' pickers read it. Panics on any other outcome.
fn close_match_uuids(outcome: &ScanOutcome) -> Vec<&str> {
    match outcome {
        ScanOutcome::CloseMatch { book, others, .. } => std::iter::once(book)
            .chain(others)
            .map(|b| b.uuid.as_str())
            .collect(),
        other => panic!("expected CloseMatch, got {other:?}"),
    }
}

/// A picked search candidate for `resolve_meta` tests.
fn picked_meta(title: &str, author: &str, isbn13: &str) -> omnibus_shared::ExternalBookMeta {
    omnibus_shared::ExternalBookMeta {
        isbn13: isbn13.into(),
        title: title.into(),
        authors: vec![author.into()],
        year: None,
        pages: None,
        publisher: None,
        description: None,
        cover_url: None,
        series: None,
        first_publish_year: None,
        source: MetadataProvider::OpenLibrary,
    }
}

async fn has_cover(pool: &SqlitePool, uuid: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT has_cover FROM books WHERE uuid = ?1")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap()
        != 0
}
