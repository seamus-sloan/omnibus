//! Tests for the shared Kobo device-name predicate: the RPC boundary and any
//! client pre-check both call it, so the accept/reject boundary is pinned here
//! once rather than re-derived per caller.

use super::{kobo_device_name_invalid, KOBO_DEVICE_NAME_MAX_LEN};

#[test]
fn kobo_device_name_invalid_accepts_an_ordinary_name() {
    assert!(!kobo_device_name_invalid("Kobo Clara"));
}

#[test]
fn kobo_device_name_invalid_accepts_a_name_at_the_maximum_length() {
    let name = "k".repeat(KOBO_DEVICE_NAME_MAX_LEN);
    assert!(!kobo_device_name_invalid(&name));
}

#[test]
fn kobo_device_name_invalid_accepts_a_padded_name_that_fits_once_trimmed() {
    let name = format!("  {}  ", "k".repeat(KOBO_DEVICE_NAME_MAX_LEN));
    assert!(!kobo_device_name_invalid(&name));
}

#[test]
fn kobo_device_name_invalid_rejects_an_empty_name() {
    assert!(kobo_device_name_invalid(""));
}

#[test]
fn kobo_device_name_invalid_rejects_a_whitespace_only_name() {
    assert!(kobo_device_name_invalid("   \t\n "));
}

#[test]
fn kobo_device_name_invalid_rejects_a_name_over_the_maximum_length() {
    let name = "k".repeat(KOBO_DEVICE_NAME_MAX_LEN + 1);
    assert!(kobo_device_name_invalid(&name));
}

#[test]
fn kobo_device_name_invalid_measures_bytes_not_characters() {
    // The cap is a byte length, so a multi-byte label crosses it sooner than
    // its character count suggests — "é" is two bytes.
    let name = "é".repeat(KOBO_DEVICE_NAME_MAX_LEN / 2 + 1);
    assert!(name.chars().count() <= KOBO_DEVICE_NAME_MAX_LEN);
    assert!(kobo_device_name_invalid(&name));
}
