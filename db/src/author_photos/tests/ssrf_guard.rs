//! The SSRF guard: `is_blocked_address` refuses loopback, RFC1918,
//! link-local, multicast, documentation, CGNAT, benchmarking, IPv6 ULA and
//! IPv4-mapped ranges while allowing public addresses, and
//! `fetch_remote_image` refuses each blocked literal before any HTTP
//! traffic flies.

use super::super::remote::{
    fetch_remote_image_with, is_blocked_address, FetchRemoteImageError, RemoteImageConfig,
};

// Issue #275 — SSRF guard. `is_blocked_address` and
// `fetch_remote_image` must refuse to open a TCP connect against the
// loopback / RFC1918 / link-local / multicast / IPv6 ULA address
// space *before* any HTTP traffic flies.
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
