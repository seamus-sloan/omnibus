//! The exact-identifier rung: an in-library ISBN resolves to unowned,
//! already-owned or on-wishlist, ignores another user's wishlist, tolerates
//! URN schemes and separators, the identifier arm wins over a copy's ISBN,
//! and `check_in_copy` canonicalizes or drops the typed ISBN.

use omnibus_shared::physical::WishlistSource;
use omnibus_shared::scan::ScanOutcome;
use wiremock::MockServer;

use crate::physical::{add_physical_copy, add_wishlist_entry, PhysicalError};

use super::super::*;
use super::{config_for, pool, seed_book, seed_user, ISBN, USER_ID};

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
