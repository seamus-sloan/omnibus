//! Tests for the two pieces of [`super::init_tracing`] that carry logic: the
//! `RUST_LOG` fallback and the log-directory failure branch.
//!
//! `init_tracing` itself is deliberately not called here. It installs a
//! *process-global* subscriber — including [`super::ErrorRingLayer`], which
//! writes into one static ring buffer — so a single call would leak into every
//! other test in this binary (the `error_ring_layer` tests assert exact buffer
//! contents, and `http_errors::internal` emits an ERROR event). Both branches
//! are covered through the helpers instead, which is why they exist as
//! standalone functions.

use omnibus_db::test_support::EnvVarGuard;

use super::{env_filter, rolling_writer, DEFAULT_FILTER};

/// `DEFAULT_FILTER` as `EnvFilter` renders it, so the assertions don't assume
/// the directive string round-trips verbatim.
fn default_filter_text() -> String {
    tracing_subscriber::EnvFilter::new(DEFAULT_FILTER).to_string()
}

#[test]
fn env_filter_uses_rust_log_when_it_is_set_and_parsable() {
    let _env = EnvVarGuard::set("RUST_LOG", Some("warn,omnibus=trace"));

    let filter = env_filter().to_string();
    assert!(
        filter.contains("omnibus=trace"),
        "unexpected filter: {filter}"
    );
    assert!(
        !filter.contains("omnibus=debug"),
        "default leaked into an explicit RUST_LOG: {filter}"
    );
}

#[test]
fn env_filter_falls_back_to_the_default_when_rust_log_is_unset() {
    let _env = EnvVarGuard::set("RUST_LOG", None);

    let filter = env_filter().to_string();
    assert!(
        filter.contains("omnibus=debug"),
        "unexpected filter: {filter}"
    );
    assert_eq!(filter, default_filter_text());
}

#[test]
fn env_filter_falls_back_to_the_default_when_rust_log_is_unparsable() {
    let _env = EnvVarGuard::set("RUST_LOG", Some("omnibus=notalevel"));

    let filter = env_filter().to_string();
    assert_eq!(filter, default_filter_text());
}

#[test]
fn rolling_writer_creates_the_log_directory_and_returns_a_guard() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("nested/logs");

    let writer = rolling_writer(&dir);

    assert!(
        writer.is_some(),
        "writer should come up for a creatable dir"
    );
    assert!(dir.is_dir(), "the log dir should have been created");
}

#[test]
fn rolling_writer_returns_none_when_the_log_directory_cannot_be_created() {
    let tmp = tempfile::tempdir().unwrap();
    // A regular file as the parent: `create_dir_all` fails on it for any user,
    // including root, so this branch is reachable in every environment.
    let blocker = tmp.path().join("not-a-dir");
    std::fs::write(&blocker, b"x").unwrap();

    assert!(rolling_writer(&blocker.join("logs")).is_none());
}
