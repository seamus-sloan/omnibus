//! `fetch_remote_image` on the wire: invalid URLs and non-HTTP schemes,
//! redirects to private addresses, non-image and SVG content types, an
//! oversized `Content-Length`, and the loopback allowance under the test
//! config.

use wiremock::{matchers::method, Mock, MockServer, ResponseTemplate};

use super::super::remote::{fetch_remote_image_with, FetchRemoteImageError, RemoteImageConfig};

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
        ..RemoteImageConfig::default()
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
        ..RemoteImageConfig::default()
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
        ..RemoteImageConfig::default()
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
    let body = vec![0xFFu8; (super::super::remote::REMOTE_IMAGE_MAX_BYTES as usize) + 1];
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
        ..RemoteImageConfig::default()
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
        ..RemoteImageConfig::default()
    };
    let err = fetch_remote_image_with("http://127.0.0.1:1/x", &cfg)
        .await
        .expect_err("nothing is listening on :1");
    assert!(
        matches!(err, FetchRemoteImageError::Http(_)),
        "lax config must skip the SSRF guard and reach the network layer, got {err:?}",
    );
}
