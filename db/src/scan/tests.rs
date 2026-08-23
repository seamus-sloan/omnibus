//! Scan-resolution tests: each ladder rung + outcome variant (AC1), CloseMatch
//! stays a candidate (AC2), and the write composition (add physical-only,
//! wishlist by uuid / by meta).

use std::time::Duration;

use omnibus_shared::metadata_lookup::MetadataProvider;
use omnibus_shared::physical::WishlistSource;
use omnibus_shared::scan::ScanOutcome;
use omnibus_shared::{Contributor, MetadataOverrides};
use serde_json::json;
use sqlx::SqlitePool;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::author_photos::RemoteImageConfig;
use crate::covers::find_cover_file;
use crate::metadata_lookup::{
    provider_cover_image_config, MetadataLookupConfig, MetadataLookupError, ProviderKeys,
};
use crate::normalize::{normalize_author, normalize_title};
use crate::physical::{
    add_physical_copy, add_wishlist_entry, list_physical_copies, list_wishlist, PhysicalError,
};
use crate::test_support::CoversTempDir;

use super::resolve::{add_physical_only_with, wishlist_add_with, MAX_CLOSE_MATCH_CANDIDATES};
use super::*;

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

/// Mount Open Library to cleanly miss, leaving Google Books unmounted so the
/// caller can wire its own (error) response.
async fn mount_ol_miss(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/api/books"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({})))
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

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
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

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        ScanOutcome::AlreadyOwned { book } if book.has_physical
    ));
}

#[tokio::test]
async fn resolve_exact_isbn_returns_on_wishlist_when_caller_wishlists_it() {
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", Some(ISBN)).await;
    let user = seed_user(&pool, "reader").await;
    add_wishlist_entry(&pool, user, "u1", WishlistSource::Manual)
        .await
        .unwrap();
    let server = MockServer::start().await; // must not be hit

    let outcome = resolve_scan(&pool, user, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(
        matches!(outcome, ScanOutcome::OnWishlist { book } if book.uuid == "u1"),
        "a wishlisted book should route to its detail page, not the check-in confirm",
    );
}

#[tokio::test]
async fn resolve_exact_isbn_ignores_another_users_wishlist() {
    // The wishlist is per-user: a book someone *else* wishlists is still an
    // InLibraryUnowned confirm for this caller.
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", Some(ISBN)).await;
    let owner = seed_user(&pool, "owner").await;
    let other = seed_user(&pool, "other").await;
    add_wishlist_entry(&pool, owner, "u1", WishlistSource::Manual)
        .await
        .unwrap();
    let server = MockServer::start().await;

    let outcome = resolve_scan(&pool, other, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(matches!(outcome, ScanOutcome::InLibraryUnowned { .. }));
}

#[tokio::test]
async fn resolve_exact_isbn_leaves_book_isbn_unset() {
    // find_book_by_isbn no longer computes isbn — nothing on this rung reads it.
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", Some(ISBN)).await;
    let server = MockServer::start().await; // must not be hit

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    match outcome {
        ScanOutcome::InLibraryUnowned { book } => assert!(book.isbn.is_none()),
        other => panic!("expected InLibraryUnowned, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_exact_isbn_tolerates_urn_scheme_and_separators() {
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", None).await;
    // OPF-style identifier: free-form scheme + hyphenated value.
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'u1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_identifiers (book_id, scheme, value)
         VALUES (?1, 'urn:isbn', '978-0-13-468599-1')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    let server = MockServer::start().await; // must not be hit

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        ScanOutcome::InLibraryUnowned { book } if book.uuid == "u1"
    ));
}

#[tokio::test]
async fn resolve_finds_the_book_a_hand_linked_copy_was_checked_in_against() {
    // The link-an-existing-book escape hatch: the barcode belongs to no
    // `book_identifiers` row, so only the physical-copy arm can bridge it —
    // and it must, or the reader is asked the same question on every re-scan.
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", None).await;
    check_in_copy(&pool, "u1", Some(ISBN), None, None)
        .await
        .unwrap();
    let server = MockServer::start().await; // must not be hit

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        ScanOutcome::AlreadyOwned { book } if book.uuid == "u1"
    ));
}

#[tokio::test]
async fn resolve_prefers_the_identifier_arm_over_a_copys_isbn() {
    // Two books claim the ISBN: one publishes it, the other only holds a copy
    // filed under it. The published identifier is the stronger claim.
    let pool = pool().await;
    seed_book(
        &pool,
        "published",
        "Effective Java",
        "Joshua Bloch",
        Some(ISBN),
    )
    .await;
    seed_book(&pool, "linked", "A Different Book", "Ada Lovelace", None).await;
    check_in_copy(&pool, "linked", Some(ISBN), None, None)
        .await
        .unwrap();
    let server = MockServer::start().await; // must not be hit

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        ScanOutcome::InLibraryUnowned { book } if book.uuid == "published"
    ));
}

#[tokio::test]
async fn check_in_copy_stores_a_typed_isbn10_in_its_canonical_form() {
    // The ladder compares against a canonical ISBN-13, so a copy filed from a
    // keypad-entered ISBN-10 must be stored folded or it is unfindable.
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", None).await;

    let copy = check_in_copy(&pool, "u1", Some("0-13-468599-7"), None, None)
        .await
        .unwrap();
    assert_eq!(copy.isbn.as_deref(), Some(ISBN));

    let server = MockServer::start().await; // must not be hit
    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(matches!(
        outcome,
        ScanOutcome::AlreadyOwned { book } if book.uuid == "u1"
    ));
}

#[tokio::test]
async fn check_in_copy_drops_an_isbn_that_does_not_validate() {
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", None).await;

    let copy = check_in_copy(&pool, "u1", Some("not-an-isbn"), None, None)
        .await
        .unwrap();
    assert_eq!(copy.isbn, None, "a wrong identifier is worse than none");
}

#[tokio::test]
async fn check_in_copy_errors_when_the_book_is_missing() {
    let pool = pool().await;
    let err = check_in_copy(&pool, "nope", Some(ISBN), None, None)
        .await
        .unwrap_err();
    assert!(matches!(
        err,
        ScanError::Physical(PhysicalError::BookNotFound)
    ));
}

#[tokio::test]
async fn resolve_close_match_via_online_then_norm() {
    let pool = pool().await;
    // Same title/author but NO matching ISBN identifier → exact rung misses.
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", None).await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "Effective Java", "Joshua Bloch").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    match outcome {
        ScanOutcome::CloseMatch {
            book,
            others,
            scanned,
        } => {
            assert_eq!(book.uuid, "u1");
            // A lone candidate leaves the tail empty, so the wire shape is the
            // one older clients already decode.
            assert!(others.is_empty());
            assert_eq!(scanned.source, MetadataProvider::OpenLibrary);
            assert_eq!(scanned.isbn13, ISBN);
        }
        other => panic!("expected CloseMatch, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_close_match_carries_the_library_editions_isbn() {
    let pool = pool().await;
    // The library copy is a different edition: same title/author, its own
    // ISBN. The 2b confirm shows both, so the match must carry it.
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", None).await;
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'u1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_identifiers (book_id, scheme, value)
         VALUES (?1, 'ISBN', '978-0-321-35668-0')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    let server = MockServer::start().await;
    mount_ol_hit(&server, "Effective Java", "Joshua Bloch").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    match outcome {
        ScanOutcome::CloseMatch { book, .. } => {
            assert_eq!(book.isbn.as_deref(), Some("9780321356680"));
        }
        other => panic!("expected CloseMatch, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_leaves_the_isbn_unset_when_the_book_has_no_thirteen_digit_one() {
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", None).await;
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = 'u1'")
        .fetch_one(&pool)
        .await
        .unwrap();
    // An ISBN-10 is not the 13-digit form the confirm screen prints.
    sqlx::query(
        "INSERT INTO book_identifiers (book_id, scheme, value)
         VALUES (?1, 'ISBN', '0321356683')",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();
    let server = MockServer::start().await;
    mount_ol_hit(&server, "Effective Java", "Joshua Bloch").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    match outcome {
        ScanOutcome::CloseMatch { book, .. } => assert!(book.isbn.is_none()),
        other => panic!("expected CloseMatch, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_close_match_uses_edited_override_title_not_scanned() {
    // The scanned OPF title is unrelated garbage, so the norm rung can only
    // match via the user's edit — the exact bug in #checkin-match-effective-title.
    let pool = pool().await;
    seed_book(
        &pool,
        "u1",
        "Garbled OPF Title",
        "Wrong Scanned Author",
        None,
    )
    .await;
    let user = seed_user(&pool, "editor").await;
    override_title_author(
        &pool,
        "u1",
        user,
        Some("The Name of the Wind"),
        Some("Patrick Rothfuss"),
    )
    .await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "The Name of the Wind", "Patrick Rothfuss").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(
        matches!(outcome, ScanOutcome::CloseMatch { book, .. } if book.uuid == "u1"),
        "edited title should feed the check-in match"
    );
}

#[tokio::test]
async fn resolve_close_match_tolerates_series_subtitle_on_scanned_title() {
    // No override: the scanned title carries the series subtitle the provider's
    // bare title lacks. The subtitle-tolerant fallback bridges the two.
    let pool = pool().await;
    seed_book(
        &pool,
        "u1",
        "The Name of the Wind (The Kingkiller Chronicle, Book 1)",
        "Patrick Rothfuss",
        None,
    )
    .await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "The Name of the Wind", "Patrick Rothfuss").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(
        matches!(outcome, ScanOutcome::CloseMatch { book, .. } if book.uuid == "u1"),
        "a trailing series subtitle should still be a close match"
    );
}

#[tokio::test]
async fn resolve_close_match_bridges_ampersand_against_spelled_out_and() {
    // The difference sits mid-string, so neither norm pass could bridge it
    // while `&` was dropped: the exact pass fails on equality and the
    // tolerant pass only widens to a word-boundary *prefix*. Check-In fell
    // through to NotInLibrary and minted a duplicate physical-only book.
    let pool = pool().await;
    seed_book(&pool, "u1", "A Tale of Mirth & Magic", "Ada Quill", None).await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "A Tale of Mirth and Magic", "Ada Quill").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(
        matches!(outcome, ScanOutcome::CloseMatch { book, .. } if book.uuid == "u1"),
        "\"&\" and \"and\" spellings must resolve to the same library book"
    );
}

#[tokio::test]
async fn resolve_norm_prefers_exact_over_ambiguous_tolerant_matches() {
    // Two same-author books: one is an exact title match, the other only a
    // tolerant (subtitle) superset. The exact pass isolates the single exact
    // candidate rather than falling through to an ambiguous tolerant pass.
    let pool = pool().await;
    seed_book(
        &pool,
        "exact",
        "The Name of the Wind",
        "Patrick Rothfuss",
        None,
    )
    .await;
    seed_book(
        &pool,
        "super",
        "The Name of the Wind Companion Guide",
        "Patrick Rothfuss",
        None,
    )
    .await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "The Name of the Wind", "Patrick Rothfuss").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(
        matches!(outcome, ScanOutcome::CloseMatch { book, .. } if book.uuid == "exact"),
        "the exact title match must win over the tolerant superset"
    );
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

#[tokio::test]
async fn resolve_close_match_offers_both_books_that_share_the_norm() {
    // Two library rows normalizing to the same (title, author) are one work in
    // two rows, not an ambiguous match: the confirm screen offers both rather
    // than declining into a duplicate physical-only book.
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", None).await;
    seed_book(&pool, "u2", "Effective Java", "Joshua Bloch", None).await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "Effective Java", "Joshua Bloch").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert_eq!(close_match_uuids(&outcome), vec!["u1", "u2"]);
}

#[tokio::test]
async fn resolve_close_match_offers_a_no_override_and_an_overridden_book_together() {
    // The live repro (#1791): an EPUB matching on `books.title_norm` and an
    // audiobook matching only through a user-edited override norm. The two
    // arms of the two-step lookup (#1343) must union into one candidate list
    // rather than counting each other out.
    let pool = pool().await;
    seed_book(&pool, "u1", "The Sword of Kaigen", "M L Wang", None).await;
    seed_book(
        &pool,
        "u2",
        "The Sword of Kaigen: A Theonite War Story",
        "M L Wang",
        None,
    )
    .await;
    let user = seed_user(&pool, "editor").await;
    override_title_author(&pool, "u2", user, Some("The Sword of Kaigen"), None).await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "The Sword of Kaigen", "M L Wang").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert_eq!(close_match_uuids(&outcome), vec!["u1", "u2"]);
}

#[tokio::test]
async fn resolve_close_match_offers_two_different_works_rather_than_picking_one() {
    // Genuinely ambiguous: two distinct books whose titles both extend the
    // scanned one, so only the tolerant pass matches and neither is the
    // obvious answer. Still a confirmation screen — never auto-resolved.
    let pool = pool().await;
    seed_book(
        &pool,
        "guide",
        "The Name of the Wind Companion Guide",
        "Patrick Rothfuss",
        None,
    )
    .await;
    seed_book(
        &pool,
        "illustrated",
        "The Name of the Wind Illustrated Edition",
        "Patrick Rothfuss",
        None,
    )
    .await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "The Name of the Wind", "Patrick Rothfuss").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert_eq!(close_match_uuids(&outcome), vec!["guide", "illustrated"]);
}

#[tokio::test]
async fn resolve_close_match_caps_the_candidate_list() {
    // A pathological match must not ship the whole shelf to a picker.
    let pool = pool().await;
    for i in 0..MAX_CLOSE_MATCH_CANDIDATES + 3 {
        seed_book(
            &pool,
            &format!("u{i}"),
            "Effective Java",
            "Joshua Bloch",
            None,
        )
        .await;
    }
    let server = MockServer::start().await;
    mount_ol_hit(&server, "Effective Java", "Joshua Bloch").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert_eq!(
        close_match_uuids(&outcome).len(),
        MAX_CLOSE_MATCH_CANDIDATES
    );
}

/// #1343: the no-override arm of `query_norm_candidates` compares
/// `books.title_norm`/`books.author_norm` directly (no `metadata_overrides`
/// join), so it must be servable by `idx_books_norm` — a `SEARCH`, not a
/// `SCAN books` — for the common case where the exact-ISBN rung misses and no
/// override exists. This is the literal WHERE clause of that arm; a
/// regression that reintroduces a `COALESCE(...)` or join on this path would
/// fail this assertion before it ever reached production.
#[tokio::test]
async fn norm_candidate_fast_path_uses_an_index_seek_not_a_table_scan() {
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", None).await;

    let plan: Vec<(i64, i64, i64, String)> = sqlx::query_as(
        "EXPLAIN QUERY PLAN
         SELECT 1 FROM books b
          WHERE b.title_norm = ?1 AND b.author_norm = ?2
            AND NOT EXISTS (SELECT 1 FROM metadata_overrides mo WHERE mo.book_uuid = b.uuid)",
    )
    .bind("effective java")
    .bind("joshua bloch")
    .fetch_all(&pool)
    .await
    .unwrap();
    let text = plan
        .iter()
        .map(|(_, _, _, s)| s.as_str())
        .collect::<Vec<_>>()
        .join(" | ");
    // Only forbid a table scan on `books`/`b` itself — whether SQLite scans or
    // seeks the (typically tiny) `metadata_overrides` table for the `NOT
    // EXISTS` probe is not the regression this test guards against. Match on
    // a trailing word boundary so a differently-aliased scan elsewhere in the
    // plan (e.g. `bal` for `books_authors_link`) can't false-positive here.
    let scans_books = plan
        .iter()
        .any(|(_, _, _, s)| s == "SCAN b" || s.starts_with("SCAN b "));
    assert!(!scans_books, "expected no table scan on books, got: {text}");
    assert!(
        text.contains("SEARCH b") && text.contains("idx_books_norm"),
        "expected an idx_books_norm index seek on books, got: {text}"
    );
}

#[tokio::test]
async fn backfill_override_norms_repairs_a_row_written_before_migration_0048() {
    // Simulate a pre-0048 override: overrides JSON present, norm columns NULL.
    // The boot backfill must populate them so the renamed book matches on
    // check-in without the user re-editing it.
    let pool = pool().await;
    seed_book(
        &pool,
        "u1",
        "Garbled OPF Title",
        "Wrong Scanned Author",
        None,
    )
    .await;
    let overrides = serde_json::to_string(&MetadataOverrides {
        title: Some("The Name of the Wind".into()),
        creators: Some(vec![Contributor {
            name: "Patrick Rothfuss".into(),
            role: None,
            file_as: None,
            id: None,
        }]),
        ..Default::default()
    })
    .unwrap();
    sqlx::query(
        "INSERT INTO metadata_overrides (book_uuid, overrides, title_norm, author_norm)
         VALUES ('u1', ?1, NULL, NULL)",
    )
    .bind(&overrides)
    .execute(&pool)
    .await
    .unwrap();

    crate::metadata_overrides::backfill_override_norm_columns(&pool)
        .await
        .unwrap();

    let (tn, an): (Option<String>, Option<String>) = sqlx::query_as(
        "SELECT title_norm, author_norm FROM metadata_overrides WHERE book_uuid = 'u1'",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(tn.as_deref(), Some("the name of the wind"));
    assert_eq!(an.as_deref(), Some("patrick rothfuss"));

    let server = MockServer::start().await;
    mount_ol_hit(&server, "The Name of the Wind", "Patrick Rothfuss").await;
    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(
        matches!(outcome, ScanOutcome::CloseMatch { book, .. } if book.uuid == "u1"),
        "backfilled override norm should make the renamed book matchable"
    );
}

#[tokio::test]
async fn resolve_not_in_library_when_online_only() {
    let pool = pool().await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "Some Other Book", "Nobody Here").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(matches!(outcome, ScanOutcome::NotInLibrary { .. }));
}

#[tokio::test]
async fn resolve_unresolved_when_both_providers_miss() {
    let pool = pool().await;
    let server = MockServer::start().await;
    mount_both_miss(&server).await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(matches!(outcome, ScanOutcome::Unresolved));
}

#[tokio::test]
async fn resolve_rejects_invalid_isbn() {
    let pool = pool().await;
    let server = MockServer::start().await;
    let err = resolve_scan(&pool, USER_ID, "12345", &config_for(&server))
        .await
        .unwrap_err();
    assert!(matches!(err, ScanError::Isbn(_)));
}

// ── resolve_meta (title-search pick) ───────────────────────────────

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

#[tokio::test]
async fn resolve_meta_returns_library_outcome_on_an_exact_isbn_hit() {
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", Some(ISBN)).await;
    let server = MockServer::start().await; // enrichment 404s are best-effort

    let outcome = resolve_meta(
        &pool,
        USER_ID,
        &picked_meta("Effective Java", "Joshua Bloch", ISBN),
        &config_for(&server),
    )
    .await
    .unwrap();
    assert!(matches!(
        outcome,
        ScanOutcome::InLibraryUnowned { book } if book.uuid == "u1"
    ));
}

#[tokio::test]
async fn resolve_meta_close_match_via_norm_without_a_provider_round_trip() {
    let pool = pool().await;
    // Library edition has a different ISBN, so only the norm rung bridges.
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", None).await;
    let server = MockServer::start().await; // no lookup mocks: must not re-resolve

    let outcome = resolve_meta(
        &pool,
        USER_ID,
        &picked_meta("Effective Java", "Joshua Bloch", ISBN),
        &config_for(&server),
    )
    .await
    .unwrap();
    match outcome {
        ScanOutcome::CloseMatch {
            book,
            others,
            scanned,
        } => {
            assert_eq!(book.uuid, "u1");
            assert!(others.is_empty());
            assert_eq!(scanned.isbn13, ISBN);
        }
        other => panic!("expected CloseMatch, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_meta_not_in_library_when_nothing_matches() {
    let pool = pool().await;
    let server = MockServer::start().await;

    let outcome = resolve_meta(
        &pool,
        USER_ID,
        &picked_meta("Some Other Book", "Nobody Here", ISBN),
        &config_for(&server),
    )
    .await
    .unwrap();
    assert!(matches!(
        outcome,
        ScanOutcome::NotInLibrary { online } if online.isbn13 == ISBN
    ));
}

#[tokio::test]
async fn resolve_meta_skips_the_exact_rung_on_an_invalid_isbn() {
    // A junk wire ISBN must not reach SQL — but the norm rung still matches.
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", None).await;
    let server = MockServer::start().await;

    let outcome = resolve_meta(
        &pool,
        USER_ID,
        &picked_meta("Effective Java", "Joshua Bloch", "garbage"),
        &config_for(&server),
    )
    .await
    .unwrap();
    assert!(matches!(
        outcome,
        ScanOutcome::CloseMatch { book, .. } if book.uuid == "u1"
    ));
}

#[tokio::test]
async fn resolve_meta_makes_no_enrichment_request_for_an_unvalidatable_isbn() {
    // `meta` is untrusted wire input and enrichment interpolates the ISBN into
    // a provider URL path, so a value that fails canonicalization must never
    // reach an outbound request — a path-traversal payload would otherwise
    // address whatever endpoint it liked on the provider's host.
    let pool = pool().await;
    let server = MockServer::start().await;
    // Any request at all to the provider host fails this test.
    Mock::given(wiremock::matchers::any())
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let outcome = resolve_meta(
        &pool,
        USER_ID,
        &picked_meta("Effective Java", "Joshua Bloch", "../../../admin/secrets"),
        &config_for(&server),
    )
    .await
    .unwrap();
    assert!(matches!(outcome, ScanOutcome::NotInLibrary { .. }));
    server.verify().await;
}

#[tokio::test]
async fn resolve_meta_enriches_the_pick_with_series_and_first_publish_year() {
    let pool = pool().await;
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path(format!("/isbn/{ISBN}.json")))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "series": ["The Java Series"],
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/search.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "docs": [{ "first_publish_year": 2001 }],
        })))
        .mount(&server)
        .await;

    let outcome = resolve_meta(
        &pool,
        USER_ID,
        &picked_meta("Effective Java", "Joshua Bloch", ISBN),
        &config_for(&server),
    )
    .await
    .unwrap();
    match outcome {
        ScanOutcome::NotInLibrary { online } => {
            assert_eq!(online.series.as_deref(), Some("The Java Series"));
            assert_eq!(online.first_publish_year, Some(2001));
        }
        other => panic!("expected NotInLibrary, got {other:?}"),
    }
}

#[tokio::test]
async fn resolve_meta_surfaces_sqlx_error_when_pool_is_closed() {
    let pool = pool().await;
    pool.close().await;
    let server = MockServer::start().await;

    let err = resolve_meta(
        &pool,
        USER_ID,
        &picked_meta("Effective Java", "Joshua Bloch", ISBN),
        &config_for(&server),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, ScanError::Sqlx(_)), "got {err:?}");
}

// ── write composition ──────────────────────────────────────────────

/// [`provider_cover_image_config`] aimed at a loopback `wiremock` origin: the
/// catalog allowlist plus `127.0.0.1`, and plaintext permitted, since a mock
/// server is neither publicly routable nor HTTPS. Every other gate — the
/// redirect budget above all — is production's, so what these tests exercise
/// is the config the check-in path actually ships.
fn loopback_cover_config() -> RemoteImageConfig {
    let mut config = provider_cover_image_config(true);
    config.require_https = false;
    config.host_allowlist.push("127.0.0.1".into());
    config
}

/// Serve a cover the way Open Library's CDN does: a 302 before the bytes.
async fn mount_redirected_cover(server: &MockServer) {
    Mock::given(method("GET"))
        .and(path("/cover.jpg"))
        .respond_with(ResponseTemplate::new(302).insert_header("location", "/cdn/cover.jpg"))
        .mount(server)
        .await;
    Mock::given(method("GET"))
        .and(path("/cdn/cover.jpg"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/jpeg")
                .set_body_bytes(b"\xFF\xD8\xFFcoverbytes".to_vec()),
        )
        .mount(server)
        .await;
}

async fn has_cover(pool: &SqlitePool, uuid: &str) -> bool {
    sqlx::query_scalar::<_, i64>("SELECT has_cover FROM books WHERE uuid = ?1")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap()
        != 0
}

#[tokio::test]
async fn add_physical_only_creates_fileless_book_with_copy_and_provider_cover() {
    let pool = pool().await;
    let _covers = CoversTempDir::new("add_physical_only_cover");
    let origin = MockServer::start().await;
    mount_redirected_cover(&origin).await;
    let mut meta = picked_meta("Print Only", "Jane Doe", ISBN);
    meta.cover_url = Some(format!("{}/cover.jpg", origin.uri()));

    let uuid = add_physical_only_with(
        &pool,
        &meta,
        Some("first ed"),
        None,
        &loopback_cover_config(),
    )
    .await
    .unwrap();

    let copies = list_physical_copies(&pool, &uuid).await.unwrap();
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].isbn.as_deref(), Some(ISBN));
    // The cover the check-in card rendered survives the redirect hop onto disk.
    assert!(has_cover(&pool, &uuid).await);
    assert!(find_cover_file(&uuid).is_some());
    // The book resolves by its ISBN now (exact rung).
    let server = MockServer::start().await;
    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(matches!(outcome, ScanOutcome::AlreadyOwned { .. }));
}

#[tokio::test]
async fn add_physical_only_refuses_a_cover_url_outside_the_provider_host_allowlist() {
    let pool = pool().await;
    let _covers = CoversTempDir::new("add_physical_only_off_catalog");
    let origin = MockServer::start().await;
    mount_redirected_cover(&origin).await;
    // The same reachable origin under a host the catalog never publishes, so
    // the allowlist is the only gate that can account for the refusal.
    let mut meta = picked_meta("Off Catalog", "Jane Doe", ISBN);
    meta.cover_url = Some(format!(
        "http://localhost:{}/cover.jpg",
        origin.address().port()
    ));

    let uuid = add_physical_only_with(&pool, &meta, None, None, &loopback_cover_config())
        .await
        .unwrap();

    assert!(!has_cover(&pool, &uuid).await);
    assert!(find_cover_file(&uuid).is_none());
    // Refused before any request left the process, not after reading bytes.
    assert!(origin.received_requests().await.unwrap().is_empty());
    // Still a book with a copy — a dropped cover must not fail the check-in.
    assert_eq!(list_physical_copies(&pool, &uuid).await.unwrap().len(), 1);
}

#[tokio::test]
async fn add_physical_only_creates_the_book_when_the_cover_fetch_fails() {
    let pool = pool().await;
    let _covers = CoversTempDir::new("add_physical_only_cover_404");
    let origin = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/cover.jpg"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&origin)
        .await;
    let mut meta = picked_meta("Missing Cover", "Jane Doe", ISBN);
    meta.cover_url = Some(format!("{}/cover.jpg", origin.uri()));

    let uuid = add_physical_only_with(&pool, &meta, None, None, &loopback_cover_config())
        .await
        .unwrap();

    assert!(!has_cover(&pool, &uuid).await);
    assert_eq!(list_physical_copies(&pool, &uuid).await.unwrap().len(), 1);
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
async fn wishlist_add_by_meta_creates_fileless_book_with_provider_cover() {
    let pool = pool().await;
    let _covers = CoversTempDir::new("wishlist_add_cover");
    let user = seed_user(&pool, "reader").await;
    let origin = MockServer::start().await;
    mount_redirected_cover(&origin).await;
    let mut meta = picked_meta("Wishlisted", "Jane Doe", ISBN);
    meta.source = MetadataProvider::GoogleBooks;
    meta.cover_url = Some(format!("{}/cover.jpg", origin.uri()));

    let uuid = wishlist_add_with(
        &pool,
        user,
        None,
        Some(&meta),
        WishlistSource::Detail,
        &loopback_cover_config(),
    )
    .await
    .unwrap();

    assert!(has_cover(&pool, &uuid).await);
    assert!(find_cover_file(&uuid).is_some());
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

// ── remaining ScanError variants (#1263) ───────────────────────────

#[tokio::test]
async fn resolve_scan_surfaces_lookup_error_when_both_providers_fail() {
    let pool = pool().await;
    let server = MockServer::start().await;
    mount_ol_miss(&server).await;
    // A non-retryable Google Books status fails the fallback immediately,
    // rather than a clean miss — the real code path behind ScanError::Lookup.
    Mock::given(method("GET"))
        .and(path("/books/v1/volumes"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let err = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap_err();
    assert!(
        matches!(err, ScanError::Lookup(MetadataLookupError::Provider(_))),
        "got {err:?}"
    );
}

#[tokio::test]
async fn wishlist_add_surfaces_physical_error_when_book_uuid_is_unresolvable() {
    let pool = pool().await;
    let user = seed_user(&pool, "reader").await;

    // No book carries this uuid, so add_wishlist_entry's canonical-uuid
    // resolution fails — the real code path behind ScanError::Physical.
    let err = wishlist_add(&pool, user, Some("nope"), None, WishlistSource::Manual)
        .await
        .unwrap_err();
    assert!(
        matches!(err, ScanError::Physical(PhysicalError::BookNotFound)),
        "got {err:?}"
    );
}

#[tokio::test]
async fn resolve_scan_surfaces_sqlx_error_when_pool_is_closed() {
    let pool = pool().await;
    pool.close().await;
    let server = MockServer::start().await; // must not be hit — the exact rung fails first

    let err = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap_err();
    assert!(matches!(err, ScanError::Sqlx(_)), "got {err:?}");
}
