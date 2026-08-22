//! Unit tests for `logging`'s two extracted-for-testability helpers:
//! `resolve_env_filter` (the `RUST_LOG` fallback, unset and unparsable) and
//! `build_file_writer` (the log-dir-creation failure branch). Neither helper
//! touches the global tracing registry, so — unlike `init_tracing` itself —
//! these tests can't race `error_ring_layer`'s tests over the process-wide
//! subscriber slot and its shared ring buffer.

use omnibus_db::test_support::EnvVarGuard;

use super::*;

/// `EnvFilter::to_string()` reorders directives, so assert on presence of
/// each half rather than the exact joined string.
fn is_the_default_filter(filter: &tracing_subscriber::EnvFilter) -> bool {
    let s = filter.to_string();
    s.contains("info") && s.contains("omnibus=debug")
}

#[test]
fn resolve_env_filter_falls_back_to_the_default_directives_when_rust_log_is_unset() {
    let _env = EnvVarGuard::set("RUST_LOG", None);
    let filter = resolve_env_filter();
    assert!(is_the_default_filter(&filter));
}

#[test]
fn resolve_env_filter_falls_back_to_the_default_directives_when_rust_log_is_unparsable() {
    // A directive's level (after `=`) must be a known level keyword or a
    // number — an arbitrary word there is what actually fails to parse
    // (tracing's grammar otherwise treats a bare word as a target with an
    // implicit TRACE level, so most "garbage" strings parse successfully).
    let _env = EnvVarGuard::set("RUST_LOG", Some("omnibus=not_a_real_level"));
    // Must not panic on the unparsable value — it falls back to the same
    // default as the unset case.
    let filter = resolve_env_filter();
    assert!(is_the_default_filter(&filter));
}

#[test]
fn resolve_env_filter_uses_rust_log_when_set_to_a_valid_directive() {
    let _env = EnvVarGuard::set("RUST_LOG", Some("debug"));
    let filter = resolve_env_filter();
    assert_eq!(filter.to_string(), "debug");
}

#[test]
fn build_file_writer_returns_some_when_the_dir_is_writable() {
    let dir = tempfile::tempdir().unwrap();
    assert!(build_file_writer(dir.path()).is_some());
}

#[test]
fn build_file_writer_returns_none_when_the_dir_cannot_be_created() {
    let parent = tempfile::tempdir().unwrap();
    // A regular file where a directory component is expected: create_dir_all
    // fails with ENOTDIR rather than silently succeeding.
    let blocker = parent.path().join("blocker-file");
    std::fs::write(&blocker, b"not a directory").unwrap();
    let unusable_dir = blocker.join("logs");

    assert!(
        build_file_writer(&unusable_dir).is_none(),
        "an uncreatable log dir must disable file logging, not panic"
    );
}
