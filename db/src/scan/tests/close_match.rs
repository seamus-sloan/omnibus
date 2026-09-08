//! The CloseMatch rung: online-then-norm matching, the ISBN carried from
//! the library edition, override titles over scanned ones, series-subtitle
//! and ampersand tolerance, the author name forms print editions carry,
//! exact-over-tolerant preference, and the loose pass's cap.

use omnibus_shared::metadata_lookup::MetadataProvider;
use omnibus_shared::scan::ScanOutcome;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::super::resolve::{LOOSE_FETCH_LIMIT, MAX_CLOSE_MATCH_CANDIDATES};
use super::super::*;
use super::{
    close_match_uuids, config_for, mount_ol_hit, override_title_author, pool, seed_book, seed_user,
    ISBN, USER_ID,
};

/// Mount Open Library to resolve `ISBN` to a book with a title but no authors
/// at all — an edition record shape Open Library genuinely publishes.
async fn mount_ol_hit_without_authors(server: &MockServer, title: &str) {
    Mock::given(method("GET"))
        .and(path("/api/books"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            format!("ISBN:{ISBN}"): { "title": title }
        })))
        .mount(server)
        .await;
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

/// The name forms one book's print editions actually carry: the library holds
/// the Gutenberg EPUB, and each provider answer is a real Google Books or Open
/// Library record for a physical edition of it (#2334). Passes 1 and 2 reject
/// every row but the first — the author key is an equality test and the title
/// key only absorbs a trailing subtitle — so each of these was a duplicate
/// physical-only book rather than a match.
#[tokio::test]
async fn resolve_close_match_bridges_the_name_forms_print_editions_carry() {
    for (title, author) in [
        ("A Room with a View", "E. M. Forster"),
        // Google Books, ISBN 014043173X — the given name spelled out.
        ("A Room with a View", "Edward Morgan Forster"),
        // Google Books, ISBN 9781548791292 — the middle initial dropped.
        ("A Room with a View", "E. Forster"),
        // Open Library, ISBN 9780486112664 — the leading article dropped.
        ("Room with a View", "E. M. Forster"),
        // Open Library, ISBN 9781548791292 — both at once.
        ("Room with a View", "E. Forster"),
    ] {
        let pool = pool().await;
        seed_book(&pool, "u1", "A Room with a View", "E. M. Forster", None).await;
        let server = MockServer::start().await;
        mount_ol_hit(&server, title, author).await;

        let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
            .await
            .unwrap();
        assert!(
            matches!(&outcome, ScanOutcome::CloseMatch { book, .. } if book.uuid == "u1"),
            "{title:?} by {author:?} should match the library EPUB, got {outcome:?}"
        );
    }
}

#[tokio::test]
async fn resolve_close_match_matches_on_title_when_the_provider_has_no_author() {
    // An Open Library edition record with an empty `authors` array used to
    // short-circuit the whole norm rung, discarding a usable title.
    let pool = pool().await;
    seed_book(&pool, "u1", "Effective Java", "Joshua Bloch", None).await;
    let server = MockServer::start().await;
    mount_ol_hit_without_authors(&server, "Effective Java").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(
        matches!(&outcome, ScanOutcome::CloseMatch { book, .. } if book.uuid == "u1"),
        "an authorless provider record should still match on title, got {outcome:?}"
    );
}

#[tokio::test]
async fn resolve_norm_does_not_match_a_different_last_name() {
    // The loose pass widens the *first* token only. Two authors who share a
    // title and nothing else must not be offered as the same book.
    let pool = pool().await;
    seed_book(&pool, "u1", "Middlemarch", "George Eliot", None).await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "Middlemarch", "George Orwell").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert!(
        matches!(outcome, ScanOutcome::NotInLibrary { .. }),
        "a different last name is not a close match"
    );
}

#[tokio::test]
async fn resolve_norm_prefers_the_exact_author_over_a_compatible_name_form() {
    // Both books carry the same title; only one carries the provider's exact
    // author key. Passes 1 and 2 isolate it before the loose pass can offer
    // the other alongside it.
    let pool = pool().await;
    seed_book(&pool, "exact", "A Room with a View", "E. M. Forster", None).await;
    seed_book(&pool, "loose", "A Room with a View", "Edward Forster", None).await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "A Room with a View", "E. M. Forster").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert_eq!(close_match_uuids(&outcome), vec!["exact"]);
}

#[tokio::test]
async fn resolve_loose_pass_caps_the_candidate_list() {
    // The loose pass fetches a wider window than it ships, so the cap has to
    // hold after the author filter thins it — not before.
    let pool = pool().await;
    for i in 0..MAX_CLOSE_MATCH_CANDIDATES + 3 {
        seed_book(
            &pool,
            &format!("u{i}"),
            "A Room with a View",
            "E. M. Forster",
            None,
        )
        .await;
    }
    let server = MockServer::start().await;
    mount_ol_hit(&server, "Room with a View", "Edward Morgan Forster").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert_eq!(
        close_match_uuids(&outcome).len(),
        MAX_CLOSE_MATCH_CANDIDATES
    );
}

#[tokio::test]
async fn resolve_loose_pass_does_not_let_same_title_strangers_crowd_out_the_match() {
    // The real book is one row among a shelf's worth of same-title books by
    // other authors. Capping the fetch before the author filter would drop it.
    let pool = pool().await;
    for i in 0..LOOSE_FETCH_LIMIT {
        seed_book(
            &pool,
            &format!("stranger-{i:03}"),
            "A Room with a View",
            &format!("Author {i:03}"),
            None,
        )
        .await;
    }
    // Sorts last by uuid, so a pre-filter cap would never reach it.
    seed_book(
        &pool,
        "zz-match",
        "A Room with a View",
        "E. M. Forster",
        None,
    )
    .await;
    let server = MockServer::start().await;
    mount_ol_hit(&server, "Room with a View", "Edward Morgan Forster").await;

    let outcome = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap();
    assert_eq!(close_match_uuids(&outcome), vec!["zz-match"]);
}
