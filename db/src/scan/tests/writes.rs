//! The write composition: `add_physical_only_with` creating a fileless
//! book with a copy and a provider cover (allowlisted host, fetch failure
//! tolerated), and `wishlist_add_with` by uuid or by picked metadata with
//! its missing-target and unresolvable-uuid errors.

use omnibus_shared::metadata_lookup::MetadataProvider;
use omnibus_shared::physical::WishlistSource;
use omnibus_shared::scan::ScanOutcome;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::author_photos::RemoteImageConfig;
use crate::covers::find_cover_file;
use crate::metadata_lookup::provider_cover_image_config;
use crate::physical::{list_physical_copies, list_wishlist, PhysicalError};
use crate::test_support::CoversTempDir;

use super::super::resolve::{add_physical_only_with, wishlist_add_with};
use super::super::*;
use super::{config_for, has_cover, picked_meta, pool, seed_book, seed_user, ISBN, USER_ID};

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
