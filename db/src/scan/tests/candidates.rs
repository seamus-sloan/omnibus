//! The candidate list and its helpers: `authors_compatible` and
//! `strip_leading_article`, CloseMatch offering every book that shares the
//! norm (capped), the norm fast path's index seek, and the override-norm
//! backfill for rows written before migration 0048.

use omnibus_shared::scan::ScanOutcome;
use omnibus_shared::{Contributor, MetadataOverrides};
use wiremock::MockServer;

use super::super::resolve::{
    authors_compatible, strip_leading_article, MAX_CLOSE_MATCH_CANDIDATES,
};
use super::super::*;
use super::{
    close_match_uuids, config_for, mount_ol_hit, override_title_author, pool, seed_book, seed_user,
    ISBN, USER_ID,
};

#[test]
fn authors_compatible_accepts_initial_and_expanded_forms_of_one_name() {
    for (provider, library) in [
        ("e m forster", "e m forster"),
        ("edward morgan forster", "e m forster"),
        ("e forster", "e m forster"),
        ("e m forster", "edward morgan forster"),
        ("forster", "e m forster"),
        ("john ronald reuel tolkien", "j r r tolkien"),
    ] {
        assert!(
            authors_compatible(Some(provider), Some(library)),
            "{provider:?} should be compatible with {library:?}"
        );
    }
}

#[test]
fn authors_compatible_rejects_a_different_last_or_first_token() {
    for (provider, library) in [
        ("george orwell", "george eliot"),
        ("edmund forster", "morgan forster"),
        ("brandon sanderson", "robert jordan"),
    ] {
        assert!(
            !authors_compatible(Some(provider), Some(library)),
            "{provider:?} should not be compatible with {library:?}"
        );
    }
}

#[test]
fn authors_compatible_treats_a_missing_key_as_cant_tell_not_no_match() {
    assert!(authors_compatible(None, Some("e m forster")));
    assert!(authors_compatible(Some("e m forster"), None));
    assert!(authors_compatible(None, None));
}

#[test]
fn strip_leading_article_drops_only_a_leading_english_article() {
    assert_eq!(
        strip_leading_article("a room with a view"),
        "room with a view"
    );
    assert_eq!(
        strip_leading_article("an ember in the ashes"),
        "ember in the ashes"
    );
    assert_eq!(strip_leading_article("the hobbit"), "hobbit");
    assert_eq!(strip_leading_article("annihilation"), "annihilation");
    assert_eq!(strip_leading_article("dune"), "dune");
    // A title that is only an article keeps it, so the key never empties.
    assert_eq!(strip_leading_article("the"), "the");
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
