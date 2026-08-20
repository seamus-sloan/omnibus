//! Tests for the shared outbound-HTTP client builder: a valid `User-Agent`
//! builds a client, an unrepresentable one surfaces the builder's error, and
//! the default agent identifies this crate and version.

use super::{build_client, default_user_agent};

#[test]
fn build_client_returns_a_client_for_a_valid_user_agent() {
    assert!(build_client("omnibus-test/1.0").is_ok());
}

#[test]
fn build_client_returns_error_when_user_agent_is_not_a_valid_header_value() {
    // A newline can't be represented in a header value, so the builder records
    // the conversion failure and surfaces it from `build()`.
    assert!(build_client("omnibus\ntest").is_err());
}

#[test]
fn default_user_agent_carries_the_crate_version_and_project_url() {
    let agent = default_user_agent();

    assert!(
        agent.starts_with(&format!("omnibus/{}", env!("CARGO_PKG_VERSION"))),
        "unexpected user agent: {agent}"
    );
    assert!(agent.contains("https://github.com/sloansa/omnibus"));
}

#[test]
fn build_client_accepts_the_default_user_agent() {
    assert!(build_client(&default_user_agent()).is_ok());
}
