//! Unit tests for the content-validator recipe: its format, its field
//! order, and the never-observed sentinel.

use super::*;

#[test]
fn content_validator_formats_mtime_then_size_as_a_quoted_strong_etag() {
    // Pins the field order: distinct values so a swapped call site fails.
    assert_eq!(
        content_validator(0x05f5_e100, 0xabc).as_deref(),
        Some("\"5f5e100-abc\"")
    );
}

#[test]
fn content_validator_is_none_for_the_never_observed_sentinel() {
    assert_eq!(content_validator(0, 0), None);
}

#[test]
fn content_validator_is_some_for_an_empty_file_with_a_real_mtime() {
    // Size 0 alone is a real stat, not the sentinel.
    assert_eq!(content_validator(42, 0).as_deref(), Some("\"2a-0\""));
}

#[test]
fn content_validator_changes_when_either_component_changes() {
    let base = content_validator(100, 200);
    assert_ne!(base, content_validator(101, 200));
    assert_ne!(base, content_validator(100, 201));
}

#[test]
fn content_validator_never_emits_a_weak_prefix() {
    // `If-Range` is strong-comparison only — a `W/` would restart every
    // resumed download from zero.
    let etag = content_validator(1, 2).expect("real stat yields a validator");
    assert!(etag.starts_with('"'), "{etag} must be a strong entity tag");
}
