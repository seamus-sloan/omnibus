//! The provider-cover gates, off by default and opted into by the cover
//! fetch: `host_allowed`'s exact and wildcard matching, the HTTPS
//! requirement, redirects that stay inside or leave the allowlist, the
//! redirect limit, and `next_hop`'s downgrade, relative-location and
//! off-allowlist refusals.

use wiremock::{
    matchers::{method, path},
    Mock, MockServer, ResponseTemplate,
};

use super::super::remote::{
    fetch_remote_image_with, next_hop, FetchRemoteImageError, RemoteImageConfig,
};

// These gates are off by default (the author-photo paste-a-URL path keeps its
// original terms exactly); the provider-cover fetch opts into all three. They
// live here because this is the one SSRF implementation in the codebase — a
// second copy for covers is precisely what these knobs exist to avoid.
/// A 1×1 PNG, so a success path can get past the magic-byte checks callers
/// run on the returned bytes.
const PNG_1X1: &[u8] = &[
    0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44, 0x52,
];

/// The test config the cover path uses: every production gate except the
/// IP-range check, which a loopback wiremock origin could never pass.
fn cover_cfg(hosts: &[&str]) -> RemoteImageConfig {
    RemoteImageConfig {
        allow_private_addresses: true,
        host_allowlist: hosts.iter().map(|h| (*h).to_string()).collect(),
        require_https: true,
        max_redirects: 4,
    }
}

#[test]
fn host_allowed_matches_an_exact_entry_case_insensitively() {
    let cfg = cover_cfg(&["covers.openlibrary.org"]);
    assert!(cfg.host_allowed("covers.openlibrary.org"));
    assert!(cfg.host_allowed("COVERS.OpenLibrary.ORG"));
    // The trailing-dot form of a fully-qualified name is the same host.
    assert!(cfg.host_allowed("covers.openlibrary.org."));
    assert!(!cfg.host_allowed("openlibrary.org"));
    assert!(!cfg.host_allowed("evil.example"));
}

#[test]
fn host_allowed_rejects_a_suffix_that_is_not_a_label_boundary() {
    // The attack an unanchored `ends_with` would allow: register
    // `notopenlibrary.org` and be treated as the real thing.
    let cfg = cover_cfg(&["*.archive.org"]);
    assert!(!cfg.host_allowed("notarchive.org"));
    assert!(!cfg.host_allowed("archive.org.evil.example"));
    assert!(!cfg.host_allowed("evilarchive.org"));
}

#[test]
fn host_allowed_matches_any_subdomain_depth_under_a_wildcard() {
    let cfg = cover_cfg(&["*.archive.org"]);
    assert!(cfg.host_allowed("ia800505.us.archive.org"));
    assert!(cfg.host_allowed("web.archive.org"));
    // But not the bare parent — that has to be listed on its own, which is
    // what the catalog does.
    assert!(!cfg.host_allowed("archive.org"));
}

#[test]
fn host_allowed_refuses_a_single_label_wildcard_that_would_cover_a_whole_tld() {
    let cfg = cover_cfg(&["*.com"]);
    assert!(!cfg.host_allowed("evil.com"));
}

#[test]
fn host_allowed_allows_everything_when_no_allowlist_is_configured() {
    // The author-photo path's terms, unchanged.
    let cfg = RemoteImageConfig::default();
    assert!(cfg.host_allowed("anything.example"));
}

#[tokio::test]
async fn fetch_remote_image_rejects_a_host_outside_the_allowlist_before_connecting() {
    // No mock is mounted at all: the refusal must happen before any request,
    // so an unmounted server answering 404 can't be what "passes" this.
    // `require_https` is off so the scheme gate — which runs first — can't be
    // what refuses the loopback origin; the allowlist has to be.
    let server = MockServer::start().await;
    let cfg = RemoteImageConfig {
        require_https: false,
        ..cover_cfg(&["covers.openlibrary.org"])
    };
    let err = fetch_remote_image_with(&format!("{}/cover.jpg", server.uri()), &cfg)
        .await
        .expect_err("a host off the allowlist must be refused");
    assert!(
        matches!(&err, FetchRemoteImageError::HostNotAllowed(h) if h == "127.0.0.1"),
        "expected HostNotAllowed, got {err:?}"
    );
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        0
    );
}

#[tokio::test]
async fn fetch_remote_image_rejects_plain_http_when_https_is_required() {
    let server = MockServer::start().await;
    // The wiremock origin is `http://127.0.0.1:…`, so allowlisting its host
    // isolates the scheme gate as the only thing that can refuse it.
    let err = fetch_remote_image_with(
        &format!("{}/cover.jpg", server.uri()),
        &cover_cfg(&["127.0.0.1"]),
    )
    .await
    .expect_err("plain http must be refused when https is required");
    assert!(
        matches!(&err, FetchRemoteImageError::Validation(m) if m.contains("https")),
        "expected an https validation error, got {err:?}"
    );
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        0
    );
}

#[tokio::test]
async fn fetch_remote_image_follows_a_redirect_that_stays_inside_the_allowlist() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start.jpg"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/final.jpg"))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/final.jpg"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("content-type", "image/png")
                .set_body_bytes(PNG_1X1.to_vec()),
        )
        .mount(&server)
        .await;

    // `require_https` off for this one: the point is the follow, and a
    // loopback wiremock can't serve TLS.
    let cfg = RemoteImageConfig {
        allow_private_addresses: true,
        host_allowlist: vec!["127.0.0.1".to_string()],
        require_https: false,
        max_redirects: 4,
    };
    let (mime, bytes) = fetch_remote_image_with(&format!("{}/start.jpg", server.uri()), &cfg)
        .await
        .expect("a same-host redirect is followed");
    assert_eq!(mime, "image/png");
    assert_eq!(bytes, PNG_1X1);
}

#[tokio::test]
async fn fetch_remote_image_refuses_a_redirect_that_leaves_the_allowlist() {
    // The gadget this exists to stop: an allowlisted host answering with a
    // 302 to somewhere it was never allowed to send us.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start.jpg"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "http://169.254.169.254/latest/"),
        )
        .expect(1)
        .mount(&server)
        .await;

    let cfg = RemoteImageConfig {
        allow_private_addresses: true,
        host_allowlist: vec!["127.0.0.1".to_string()],
        require_https: false,
        max_redirects: 4,
    };
    let err = fetch_remote_image_with(&format!("{}/start.jpg", server.uri()), &cfg)
        .await
        .expect_err("a redirect off the allowlist must not be followed");
    assert!(
        matches!(&err, FetchRemoteImageError::HostNotAllowed(h) if h == "169.254.169.254"),
        "expected HostNotAllowed for the redirect target, got {err:?}"
    );
}

#[tokio::test]
async fn fetch_remote_image_refuses_an_http_url_before_it_is_ever_requested() {
    // A `wiremock` origin is plaintext loopback, so an https-required config
    // can only ever be pointed at an http URL here — which means this proves
    // the scheme gate refuses the *original* URL, not that it re-runs per
    // hop. The per-hop re-gating is proved by
    // `fetch_remote_image_refuses_a_redirect_that_leaves_the_allowlist`
    // above: `next_hop` runs the redirect target through the same
    // `parse_and_gate` that enforces both the scheme and the allowlist, so a
    // hop that fails either one ends the fetch the same way.
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/start.jpg"))
        .respond_with(ResponseTemplate::new(200))
        .expect(0)
        .mount(&server)
        .await;

    let cfg = RemoteImageConfig {
        allow_private_addresses: true,
        host_allowlist: vec!["127.0.0.1".to_string()],
        require_https: true,
        max_redirects: 4,
    };
    let err = fetch_remote_image_with(&format!("{}/start.jpg", server.uri()), &cfg)
        .await
        .expect_err("http must be refused under require_https");
    assert!(
        matches!(&err, FetchRemoteImageError::Validation(m) if m.contains("https")),
        "expected an https validation error, got {err:?}"
    );
    // Refused before any request went out, which is the point of gating on
    // the parsed URL rather than on the response.
    server.verify().await;
}

#[tokio::test]
async fn fetch_remote_image_gives_up_after_the_redirect_limit() {
    let server = MockServer::start().await;
    // A loop: every hop redirects to itself, so only the limit can end it.
    Mock::given(method("GET"))
        .and(path("/loop.jpg"))
        .respond_with(ResponseTemplate::new(302).insert_header("Location", "/loop.jpg"))
        .mount(&server)
        .await;

    let cfg = RemoteImageConfig {
        allow_private_addresses: true,
        host_allowlist: vec!["127.0.0.1".to_string()],
        require_https: false,
        max_redirects: 2,
    };
    let err = fetch_remote_image_with(&format!("{}/loop.jpg", server.uri()), &cfg)
        .await
        .expect_err("a redirect loop must terminate");
    assert!(
        matches!(&err, FetchRemoteImageError::Validation(m) if m.contains("too many redirects")),
        "expected a redirect-limit error, got {err:?}"
    );
    // The limit is hops *followed*: the original request plus two follows.
    assert_eq!(
        server.received_requests().await.unwrap_or_default().len(),
        3
    );
}

/// Build a 3xx `reqwest::Response` carrying `location`, so [`next_hop`] can
/// be driven directly.
fn redirect_to(location: &str) -> reqwest::Response {
    reqwest::Response::from(
        http::Response::builder()
            .status(302)
            .header("location", location)
            .body("")
            .expect("a 302 with a Location header is well-formed"),
    )
}

#[test]
fn next_hop_refuses_a_redirect_that_downgrades_https_to_http() {
    // The gate with no end-to-end coverage: a `wiremock` origin is plaintext,
    // so an https-required fetch is refused at hop 1 and hop 2 never happens.
    // Driving `next_hop` directly is the only way to reach it — and without
    // this, deleting the scheme check from the per-hop re-gate leaves the
    // suite green while a provider 302 to `http://<attacker>/x.png` is
    // followed in cleartext.
    let from = reqwest::Url::parse("https://covers.openlibrary.org/b/id/1-L.jpg").unwrap();
    let cfg = RemoteImageConfig {
        host_allowlist: vec!["covers.openlibrary.org".to_string()],
        require_https: true,
        max_redirects: 4,
        ..RemoteImageConfig::default()
    };

    // Same host, so only the scheme can refuse it.
    let err = next_hop(
        &from,
        &redirect_to("http://covers.openlibrary.org/x.png"),
        &cfg,
    )
    .expect_err("a downgrade to http must not be followed");
    assert!(
        matches!(&err, FetchRemoteImageError::Validation(m) if m.contains("https")),
        "expected an https validation error, got {err:?}"
    );
}

#[test]
fn next_hop_follows_a_relative_location_against_the_hop_it_came_from() {
    // Relative Locations are legal and common; resolving them against the
    // current hop is what keeps the re-gate checking the URL actually about
    // to be fetched.
    let from = reqwest::Url::parse("https://covers.openlibrary.org/b/id/1-L.jpg").unwrap();
    let cfg = RemoteImageConfig {
        host_allowlist: vec!["covers.openlibrary.org".to_string()],
        require_https: true,
        max_redirects: 4,
        ..RemoteImageConfig::default()
    };
    let next = next_hop(&from, &redirect_to("/b/id/2-L.jpg"), &cfg).expect("same-host hop");
    assert_eq!(next.as_str(), "https://covers.openlibrary.org/b/id/2-L.jpg");
}

#[test]
fn next_hop_refuses_a_redirect_to_a_host_off_the_allowlist() {
    let from = reqwest::Url::parse("https://covers.openlibrary.org/b/id/1-L.jpg").unwrap();
    let cfg = RemoteImageConfig {
        host_allowlist: vec!["covers.openlibrary.org".to_string()],
        require_https: true,
        max_redirects: 4,
        ..RemoteImageConfig::default()
    };
    let err = next_hop(&from, &redirect_to("https://evil.example/x.png"), &cfg)
        .expect_err("a hop off the allowlist must not be followed");
    assert!(
        matches!(&err, FetchRemoteImageError::HostNotAllowed(h) if h == "evil.example"),
        "expected HostNotAllowed, got {err:?}"
    );
}

#[test]
fn next_hop_refuses_a_redirect_with_no_usable_location() {
    let from = reqwest::Url::parse("https://covers.openlibrary.org/b/id/1-L.jpg").unwrap();
    let bare = reqwest::Response::from(http::Response::builder().status(302).body("").unwrap());
    let err = next_hop(&from, &bare, &RemoteImageConfig::default())
        .expect_err("a 3xx with no Location cannot be followed");
    assert!(matches!(&err, FetchRemoteImageError::Validation(m) if m.contains("Location")));
}
