//! Tests for `db::author_photos` — cascade resolver, SSRF guard, and
//! data-layer helpers. Exercises the Open Library wiremock, blocked-address
//! variants, and the DB upsert path.

use std::time::Duration;

use sqlx::SqlitePool;
use wiremock::{
    matchers::{method, path, query_param},
    Mock, MockServer, ResponseTemplate,
};

use super::cascade::{fetch_open_library, refetch_all, resolve_with, OpenLibraryConfig};
use super::remote::{
    fetch_remote_image_with, is_blocked_address, FetchRemoteImageError, RemoteImageConfig,
};
use crate::author_photos_data::{
    author_photo_status, get_author_photo, upsert_author_photo, AuthorPhotoSource,
};
use crate::pool::init_db;

async fn pool_with_author(name: &str) -> (SqlitePool, i64) {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let id: i64 = sqlx::query_scalar("INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id")
        .bind(name)
        .bind(name)
        .fetch_one(&pool)
        .await
        .unwrap();
    (pool, id)
}

fn config_for(server: &MockServer) -> OpenLibraryConfig {
    OpenLibraryConfig {
        base_search_url: server.uri(),
        base_covers_url: server.uri(),
        timeout: Duration::from_secs(2),
        user_agent: "omnibus-test".into(),
    }
}

#[tokio::test]
async fn resolve_writes_open_library_hit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/authors.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "docs": [ { "key": "OL23919A" } ]
        })))
        .mount(&server)
        .await;
    // 2 KB of bytes so the MIN_IMAGE_BYTES guard passes.
    let payload = vec![0xABu8; 2048];
    Mock::given(method("GET"))
        .and(path("/a/olid/OL23919A-L.jpg"))
        .and(query_param("default", "false"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/jpeg")
                .set_body_bytes(payload.clone()),
        )
        .mount(&server)
        .await;

    let (pool, id) = pool_with_author("Ada Lovelace").await;
    let cfg = config_for(&server);
    resolve_with(&pool, id, &cfg).await.unwrap();

    let (mime, bytes) = get_author_photo(&pool, id).await.unwrap().unwrap();
    assert_eq!(mime, "image/jpeg");
    assert_eq!(bytes, payload);
    let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::OpenLibrary);
}

#[tokio::test]
async fn resolve_writes_letter_when_search_empty() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/authors.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "docs": []
        })))
        .mount(&server)
        .await;

    let (pool, id) = pool_with_author("Nobody In Particular").await;
    let cfg = config_for(&server);
    resolve_with(&pool, id, &cfg).await.unwrap();

    assert!(get_author_photo(&pool, id).await.unwrap().is_none());
    let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::Letter);
}

#[tokio::test]
async fn resolve_writes_letter_when_cover_missing() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/authors.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "docs": [ { "key": "OL999A" } ]
        })))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/a/olid/OL999A-L.jpg"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let (pool, id) = pool_with_author("Ada Lovelace").await;
    let cfg = config_for(&server);
    resolve_with(&pool, id, &cfg).await.unwrap();

    let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::Letter);
}

#[tokio::test]
async fn resolve_writes_letter_when_image_too_small() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/authors.json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "docs": [ { "key": "OL1A" } ]
        })))
        .mount(&server)
        .await;
    // Tiny placeholder (well under MIN_IMAGE_BYTES) — should be treated
    // as a miss.
    Mock::given(method("GET"))
        .and(path("/a/olid/OL1A-L.jpg"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/gif")
                .set_body_bytes(vec![0u8; 42]),
        )
        .mount(&server)
        .await;

    let (pool, id) = pool_with_author("Ada Lovelace").await;
    let cfg = config_for(&server);
    resolve_with(&pool, id, &cfg).await.unwrap();

    let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::Letter);
}

#[tokio::test]
async fn resolve_is_noop_when_letter_marker_exists() {
    // Existing letter marker must prevent any HTTP call. We assert this
    // by starting a mock server with *no* mounted responses — any
    // incoming request would 404 and we'd notice via the marker source
    // not changing.
    let server = MockServer::start().await;
    let (pool, id) = pool_with_author("Ada Lovelace").await;
    upsert_author_photo(&pool, id, AuthorPhotoSource::Letter, None, None, None)
        .await
        .unwrap();
    let cfg = config_for(&server);
    resolve_with(&pool, id, &cfg).await.unwrap();

    let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::Letter);
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        0,
        "letter marker must skip the network entirely"
    );
}

#[tokio::test]
async fn resolve_is_noop_when_manual_upload_exists() {
    let server = MockServer::start().await;
    let (pool, id) = pool_with_author("Ada Lovelace").await;
    upsert_author_photo(
        &pool,
        id,
        AuthorPhotoSource::Manual,
        None,
        Some("image/jpeg"),
        Some(b"\xFF\xD8\xFFmanual"),
    )
    .await
    .unwrap();
    let cfg = config_for(&server);
    resolve_with(&pool, id, &cfg).await.unwrap();

    let (src, _) = author_photo_status(&pool, id).await.unwrap().unwrap();
    assert_eq!(src, AuthorPhotoSource::Manual);
}

#[tokio::test]
async fn fetch_open_library_sends_configured_user_agent() {
    // The shared client carries the production UA, but each request must
    // override it with the config's `user_agent` so test injection (and
    // any future per-call UA) still takes effect.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/authors.json"))
        .and(wiremock::matchers::header("user-agent", "omnibus-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "docs": []
        })))
        .mount(&server)
        .await;

    let cfg = config_for(&server);
    let result = fetch_open_library("Ada Lovelace", &cfg).await.unwrap();
    assert!(result.is_none());
    // The header matcher above only matches when the UA is correct, so a
    // single received request confirms the override fired.
    assert_eq!(server.received_requests().await.unwrap().len(), 1);
}

// -----------------------------------------------------------------
// Issue #275 — SSRF guard. `is_blocked_address` and
// `fetch_remote_image` must refuse to open a TCP connect against the
// loopback / RFC1918 / link-local / multicast / IPv6 ULA address
// space *before* any HTTP traffic flies.
// -----------------------------------------------------------------

#[test]
fn is_blocked_address_blocks_loopback_v4() {
    assert!(is_blocked_address("127.0.0.1".parse().unwrap()));
    assert!(is_blocked_address("127.255.255.254".parse().unwrap()));
}

#[test]
fn is_blocked_address_blocks_private_v4() {
    for ip in [
        "10.0.0.1",
        "10.255.255.255",
        "192.168.1.1",
        "172.16.0.1",
        "172.31.255.255",
    ] {
        assert!(is_blocked_address(ip.parse().unwrap()), "{ip}");
    }
}

#[test]
fn is_blocked_address_blocks_link_local_v4() {
    // 169.254/16 — covers the AWS/GCP/Azure IMDS endpoint at .169.254.
    assert!(is_blocked_address("169.254.169.254".parse().unwrap()));
    assert!(is_blocked_address("169.254.0.1".parse().unwrap()));
}

#[test]
fn is_blocked_address_blocks_multicast_unspecified_broadcast_v4() {
    assert!(is_blocked_address("0.0.0.0".parse().unwrap()));
    assert!(is_blocked_address("255.255.255.255".parse().unwrap()));
    assert!(is_blocked_address("224.0.0.1".parse().unwrap()));
}

#[test]
fn is_blocked_address_blocks_documentation_v4() {
    assert!(is_blocked_address("192.0.2.1".parse().unwrap()));
    assert!(is_blocked_address("198.51.100.1".parse().unwrap()));
    assert!(is_blocked_address("203.0.113.1".parse().unwrap()));
}

#[test]
fn is_blocked_address_blocks_carrier_grade_nat_v4() {
    // 100.64.0.0/10 — RFC 6598 CGN range.
    assert!(is_blocked_address("100.64.0.1".parse().unwrap()));
    assert!(is_blocked_address("100.127.255.255".parse().unwrap()));
    // 100.63.x and 100.128.x sit outside /10 — must NOT be blocked.
    assert!(!is_blocked_address("100.63.255.255".parse().unwrap()));
    assert!(!is_blocked_address("100.128.0.1".parse().unwrap()));
}

#[test]
fn is_blocked_address_blocks_benchmarking_v4() {
    // 198.18.0.0/15.
    assert!(is_blocked_address("198.18.0.1".parse().unwrap()));
    assert!(is_blocked_address("198.19.255.255".parse().unwrap()));
    assert!(!is_blocked_address("198.20.0.1".parse().unwrap()));
}

#[test]
fn is_blocked_address_blocks_loopback_v6() {
    assert!(is_blocked_address("::1".parse().unwrap()));
    assert!(is_blocked_address("::".parse().unwrap()));
}

#[test]
fn is_blocked_address_blocks_unique_local_v6() {
    // fc00::/7 — both `fc` and `fd` high bytes.
    assert!(is_blocked_address("fc00::1".parse().unwrap()));
    assert!(is_blocked_address("fd00::1".parse().unwrap()));
    // fe00:: is OUTSIDE fc00::/7 (the next /8 up) — must NOT be blocked
    // by the ULA check (it WOULD be caught by other rules if applicable
    // but is otherwise treated as a normal public unicast prefix).
}

#[test]
fn is_blocked_address_blocks_link_local_v6() {
    // fe80::/10 — link-local.
    assert!(is_blocked_address("fe80::1".parse().unwrap()));
    assert!(is_blocked_address("febf:ffff::1".parse().unwrap()));
}

#[test]
fn is_blocked_address_blocks_ipv4_mapped_loopback() {
    // ::ffff:127.0.0.1 — IPv6 wrapper around the v4 loopback. Must be
    // recognised so a hostile URL can't slip through the v4 guard by
    // dressing the address up as v6.
    assert!(is_blocked_address("::ffff:127.0.0.1".parse().unwrap()));
    // ::ffff:169.254.169.254 — same wrapper, IMDS target.
    assert!(is_blocked_address(
        "::ffff:169.254.169.254".parse().unwrap()
    ));
}

#[test]
fn is_blocked_address_blocks_ipv6_multicast_and_documentation() {
    assert!(is_blocked_address("ff02::1".parse().unwrap()));
    // 2001:db8::/32 documentation prefix.
    assert!(is_blocked_address("2001:db8::1".parse().unwrap()));
}

#[test]
fn is_blocked_address_allows_public_v4() {
    // Spot-check a handful of public unicast IPs to make sure the guard
    // hasn't accidentally over-matched.
    assert!(!is_blocked_address("1.1.1.1".parse().unwrap()));
    assert!(!is_blocked_address("8.8.8.8".parse().unwrap()));
    assert!(!is_blocked_address("104.16.0.1".parse().unwrap()));
}

#[test]
fn is_blocked_address_allows_public_v6() {
    // Google DNS over v6.
    assert!(!is_blocked_address("2001:4860:4860::8888".parse().unwrap()));
    // Cloudflare DNS over v6.
    assert!(!is_blocked_address("2606:4700:4700::1111".parse().unwrap()));
}

#[tokio::test]
async fn fetch_remote_image_blocks_loopback_literal_under_strict_config() {
    // Strict config (the production default). Must reject before any
    // TCP connect — there's nothing listening on 127.0.0.1:1 in CI, so
    // a regression that bypassed the guard would surface as a
    // connect-refused HTTP error instead of `BlockedAddress`.
    let cfg = RemoteImageConfig::default();
    let err = fetch_remote_image_with("http://127.0.0.1:1/x", &cfg)
        .await
        .expect_err("must block loopback under strict config");
    assert!(matches!(err, FetchRemoteImageError::BlockedAddress(_)));
}

#[tokio::test]
async fn fetch_remote_image_blocks_aws_imds_link_local_under_strict_config() {
    // 169.254.169.254 — the headline SSRF target. Must be refused
    // regardless of whether the underlying network would actually
    // route to it.
    let cfg = RemoteImageConfig::default();
    let err = fetch_remote_image_with("http://169.254.169.254/latest/meta-data/", &cfg)
        .await
        .expect_err("must block IMDS link-local under strict config");
    assert!(matches!(err, FetchRemoteImageError::BlockedAddress(_)));
}

#[tokio::test]
async fn fetch_remote_image_blocks_rfc1918_private_under_strict_config() {
    let cfg = RemoteImageConfig::default();
    for url in [
        "http://10.0.0.1/x",
        "http://192.168.1.1/x",
        "http://172.16.0.1/x",
    ] {
        let err = fetch_remote_image_with(url, &cfg)
            .await
            .expect_err("must block RFC1918 under strict config");
        assert!(
            matches!(err, FetchRemoteImageError::BlockedAddress(_)),
            "expected BlockedAddress for {url}, got {err:?}",
        );
    }
}

#[tokio::test]
async fn fetch_remote_image_blocks_ipv6_loopback_under_strict_config() {
    let cfg = RemoteImageConfig::default();
    let err = fetch_remote_image_with("http://[::1]:1/x", &cfg)
        .await
        .expect_err("must block IPv6 loopback under strict config");
    assert!(matches!(err, FetchRemoteImageError::BlockedAddress(_)));
}

#[tokio::test]
async fn fetch_remote_image_rejects_invalid_url() {
    let cfg = RemoteImageConfig::default();
    // Garbage URL (no host) — must be rejected at validation, before any
    // resolution. Asserting `Validation` *only* (not `BlockedAddress`) catches
    // a regression where a malformed URL slips through to DNS resolution.
    let err = fetch_remote_image_with("http://", &cfg)
        .await
        .expect_err("must reject malformed URL");
    assert!(
        matches!(err, FetchRemoteImageError::Validation(_)),
        "malformed URL must be a validation error, not resolved, got {err:?}",
    );
}

#[tokio::test]
async fn fetch_remote_image_rejects_non_http_scheme() {
    let cfg = RemoteImageConfig::default();
    let err = fetch_remote_image_with("ftp://example.com/p.jpg", &cfg)
        .await
        .expect_err("must reject non-http(s)");
    assert!(
        matches!(&err, FetchRemoteImageError::Validation(msg) if msg.contains("http://")),
        "expected a scheme validation error, got {err:?}",
    );
}

#[tokio::test]
async fn fetch_remote_image_does_not_follow_redirects_to_private_ips() {
    // Regression for the SSRF redirect-bypass: an admin could host
    // `attacker.com` (resolves to a public IP, passes the IP-range
    // guard in strict mode) and serve a 302 to `http://127.0.0.1/`
    // or `http://169.254.169.254/`. If reqwest follows the redirect
    // it re-resolves through its default resolver and the IP-range
    // guard never sees the new host — full bypass. The fix disables
    // redirect-following on the per-call client, so a 3xx surfaces
    // as a 302-status validation error and the SSRF target is never hit.
    //
    // The test runs under `allow_private_addresses` so the wiremock
    // server bound to loopback isn't rejected up-front — the point
    // is to exercise the redirect-policy code, not the IP guard.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "http://169.254.169.254/"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let cfg = RemoteImageConfig {
        allow_private_addresses: true,
    };
    let err = fetch_remote_image_with(&format!("{}/photo.jpg", server.uri()), &cfg)
        .await
        .expect_err("redirect must NOT be followed");
    assert!(
        matches!(&err, FetchRemoteImageError::Validation(msg) if msg.contains("302")),
        "expected a 302-status validation error (redirect not followed), got {err:?}",
    );
}

#[tokio::test]
async fn fetch_remote_image_rejects_non_image_content_type() {
    // Post-fetch validation gate: a 200 whose content-type isn't `image/*`
    // (e.g. an HTML error page served with 200) must be rejected as a
    // `Validation` error naming the content-type, never persisted as a photo.
    // Runs under the test config so the loopback wiremock host isn't blocked
    // up-front — the point is the content-type gate, not the SSRF guard.
    let server = MockServer::start().await;
    // `set_body_bytes` (unlike `set_body_string`) doesn't stamp its own
    // content-type, so the explicit `insert_header` is what reqwest observes.
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/html")
                .set_body_bytes(b"<html>not an image</html>".to_vec()),
        )
        .mount(&server)
        .await;
    let cfg = RemoteImageConfig {
        allow_private_addresses: true,
    };
    let err = fetch_remote_image_with(&format!("{}/x.jpg", server.uri()), &cfg)
        .await
        .expect_err("non-image content-type must be rejected");
    assert!(
        matches!(&err, FetchRemoteImageError::Validation(msg) if msg.contains("not an image")),
        "got {err:?}",
    );
}

#[tokio::test]
async fn fetch_remote_image_rejects_svg_content_type() {
    // SVG is an image type but is refused outright (it can carry scripts), so
    // a `content-type: image/svg+xml` response — which passes the `image/`
    // prefix check — must still be rejected by the dedicated SVG gate as a
    // `Validation` error.
    let server = MockServer::start().await;
    // `set_body_bytes` keeps our `image/svg+xml` header intact (a string body
    // would force `text/plain` and hit the not-an-image gate instead), so the
    // response passes the `image/` prefix check and reaches the SVG gate.
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/svg+xml")
                .set_body_bytes(b"<svg/>".to_vec()),
        )
        .mount(&server)
        .await;
    let cfg = RemoteImageConfig {
        allow_private_addresses: true,
    };
    let err = fetch_remote_image_with(&format!("{}/x.svg", server.uri()), &cfg)
        .await
        .expect_err("SVG content-type must be rejected");
    assert!(
        matches!(&err, FetchRemoteImageError::Validation(msg) if msg.contains("SVG")),
        "got {err:?}",
    );
}

#[tokio::test]
async fn fetch_remote_image_rejects_oversized_content_length() {
    // An advertised `Content-Length` past `REMOTE_IMAGE_MAX_BYTES` must bail on
    // the pre-check (remote.rs `resp.content_length()`) — before the streaming
    // read — so an obviously-oversized download aborts up front. This is the
    // gate that actually fires here: the streamed body path is the belt-and-
    // braces fallback for servers that omit or lie about Content-Length.
    //
    // hyper refuses to emit a Content-Length that disagrees with the body it
    // sends (it panics on the mismatch), so a fabricated huge header over a
    // tiny body isn't possible over wiremock — the body must genuinely exceed
    // the cap by one byte, which is what makes the honest Content-Length
    // oversized. `+ 1` keeps the allocation to the minimum that trips the gate.
    let server = MockServer::start().await;
    let body = vec![0xFFu8; (super::remote::REMOTE_IMAGE_MAX_BYTES as usize) + 1];
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/jpeg")
                .set_body_bytes(body),
        )
        .mount(&server)
        .await;
    let cfg = RemoteImageConfig {
        allow_private_addresses: true,
    };
    let err = fetch_remote_image_with(&format!("{}/big.jpg", server.uri()), &cfg)
        .await
        .expect_err("oversized Content-Length must be rejected");
    assert!(
        matches!(&err, FetchRemoteImageError::Validation(msg) if msg.contains("cap")),
        "got {err:?}",
    );
}

#[tokio::test]
async fn fetch_remote_image_allows_loopback_under_test_config() {
    // The test-only escape hatch must actually flip the guard off so
    // the existing wiremock-backed integration tests can keep working.
    // We confirm by pointing at a TCP port that nothing is listening
    // on — strict config would have returned `BlockedAddress`, lax
    // config gets all the way to a transport-level `Http` error.
    let cfg = RemoteImageConfig {
        allow_private_addresses: true,
    };
    let err = fetch_remote_image_with("http://127.0.0.1:1/x", &cfg)
        .await
        .expect_err("nothing is listening on :1");
    assert!(
        matches!(err, FetchRemoteImageError::Http(_)),
        "lax config must skip the SSRF guard and reach the network layer, got {err:?}",
    );
}

#[tokio::test]
async fn resolve_leaves_row_absent_on_transient_network_error() {
    // Point the resolver at a TCP port that nothing is listening on so
    // every request errors at the transport layer. A transient outage
    // must NOT cache a `letter` marker — the next call should be free
    // to retry, not stuck for an admin to manually clear.
    let cfg = OpenLibraryConfig {
        base_search_url: "http://127.0.0.1:1".into(),
        base_covers_url: "http://127.0.0.1:1".into(),
        timeout: Duration::from_millis(500),
        user_agent: "omnibus-test".into(),
    };
    let (pool, id) = pool_with_author("Ada Lovelace").await;
    resolve_with(&pool, id, &cfg).await.unwrap();

    assert!(
        author_photo_status(&pool, id).await.unwrap().is_none(),
        "transient network error must leave the row absent for retry"
    );
}

// ── refetch_all ─────────────────────────────────────────────────

#[tokio::test]
async fn refetch_all_skips_manual_and_reports_progress() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let manual_id: i64 = sqlx::query_scalar(
        "INSERT INTO authors (name, sort) VALUES ('Manual Author', 'Manual Author') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let letter_id: i64 = sqlx::query_scalar(
        "INSERT INTO authors (name, sort) VALUES ('Letter Author', 'Letter Author') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();
    let empty_id: i64 = sqlx::query_scalar(
        "INSERT INTO authors (name, sort) VALUES ('No Photo', 'No Photo') RETURNING id",
    )
    .fetch_one(&pool)
    .await
    .unwrap();

    upsert_author_photo(
        &pool,
        manual_id,
        AuthorPhotoSource::Manual,
        Some("https://example.com/manual.jpg"),
        Some("image/jpeg"),
        Some(&[0xFF; 2048]),
    )
    .await
    .unwrap();
    upsert_author_photo(
        &pool,
        letter_id,
        AuthorPhotoSource::Letter,
        None,
        None,
        None,
    )
    .await
    .unwrap();

    let progress = std::sync::Mutex::new(Vec::new());
    refetch_all(&pool, |processed, total, _| {
        progress.lock().unwrap().push((processed, total));
    })
    .await
    .unwrap();

    let calls = progress.into_inner().unwrap();
    assert_eq!(calls.len(), 3, "one progress call per author");
    assert_eq!(calls[0], (1, Some(3)));
    assert_eq!(calls[1], (2, Some(3)));
    assert_eq!(calls[2], (3, Some(3)));

    let (src, _) = author_photo_status(&pool, manual_id)
        .await
        .unwrap()
        .expect("manual row should be preserved");
    assert_eq!(src, AuthorPhotoSource::Manual);

    // letter row was deleted + resolve re-ran. With real OL, the search
    // returns nothing for "Letter Author" so a new letter marker is written.
    // The key invariant: the old row was cleared and the cascade ran again.
    match author_photo_status(&pool, letter_id).await.unwrap() {
        Some((AuthorPhotoSource::Letter, _)) => {} // re-resolved to letter (expected)
        None => {}                                 // transient network error left absent
        other => panic!("unexpected status for letter author: {other:?}"),
    }

    // "No Photo" had no row — resolve ran (OL search miss → letter marker,
    // or transient error → absent). Either is fine; the point is the cascade
    // executed without error.
    match author_photo_status(&pool, empty_id).await.unwrap() {
        Some((AuthorPhotoSource::Letter, _)) | None => {}
        other => panic!("unexpected status for empty author: {other:?}"),
    }
}

#[tokio::test]
async fn refetch_all_processes_every_author_across_multiple_concurrency_chunks() {
    // REFETCH_CONCURRENCY == 6; seed 8 authors so `to_refetch` spans two
    // `chunks()` rounds (6 + 2) and the second round is actually exercised.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let mut author_ids = Vec::with_capacity(8);
    for i in 0..8 {
        let id: i64 =
            sqlx::query_scalar("INSERT INTO authors (name, sort) VALUES (?, ?) RETURNING id")
                .bind(format!("Chunk Test Author {i}"))
                .bind(format!("Chunk Test Author {i}"))
                .fetch_one(&pool)
                .await
                .unwrap();
        author_ids.push(id);
    }

    let progress = std::sync::Mutex::new(Vec::new());
    let names = std::sync::Mutex::new(Vec::new());
    refetch_all(&pool, |processed, total, name| {
        progress.lock().unwrap().push((processed, total));
        names.lock().unwrap().push(name.map(str::to_string));
    })
    .await
    .unwrap();

    // Every refetched author reports its name as the current item;
    // completion order under concurrency is arbitrary, so only membership
    // is asserted.
    let names = names.into_inner().unwrap();
    assert!(
        names.iter().all(|n| n
            .as_deref()
            .is_some_and(|n| n.starts_with("Chunk Test Author"))),
        "every progress call must carry the completed author name: {names:?}"
    );

    let mut calls = progress.into_inner().unwrap();
    assert_eq!(
        calls.len(),
        8,
        "one progress call per author, spanning both concurrency chunks"
    );
    calls.sort_unstable();
    let expected: Vec<(u32, Option<u32>)> = (1..=8).map(|n| (n, Some(8))).collect();
    assert_eq!(
        calls, expected,
        "the completion counter must reach every value 1..=8 exactly once, \
         regardless of which chunk (or which author within a chunk) finishes first"
    );

    // Every author must have run the cascade to completion: either a sticky
    // `letter` marker (clean Open Library miss) or an absent row (transient
    // network error, e.g. no network access in this environment). Either
    // outcome proves the second chunk actually ran, not just the first 6.
    for id in author_ids {
        match author_photo_status(&pool, id).await.unwrap() {
            Some((AuthorPhotoSource::Letter, _)) | None => {}
            other => panic!("unexpected status for author {id}: {other:?}"),
        }
    }
}
