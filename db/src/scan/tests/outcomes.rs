//! The terminal `ScanOutcome` variants of `resolve_scan` (not-in-library,
//! unresolved, invalid ISBN, provider and DB failures) and the
//! `resolve_meta` entry point: library hit, norm close match without a
//! provider round trip, the invalid-ISBN rung skip, and enrichment.

use omnibus_shared::scan::ScanOutcome;
use serde_json::json;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::metadata_lookup::MetadataLookupError;

use super::super::*;
use super::{config_for, mount_ol_hit, picked_meta, pool, seed_book, ISBN, USER_ID};

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
async fn resolve_scan_surfaces_sqlx_error_when_pool_is_closed() {
    let pool = pool().await;
    pool.close().await;
    let server = MockServer::start().await; // must not be hit — the exact rung fails first

    let err = resolve_scan(&pool, USER_ID, ISBN, &config_for(&server))
        .await
        .unwrap_err();
    assert!(matches!(err, ScanError::Sqlx(_)), "got {err:?}");
}
