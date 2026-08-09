//! Tests for [`super::ErrorRingLayer`]: an `ERROR` event must be captured
//! into the ring buffer with no explicit call site (AC1/AC3); any other
//! level must be a no-op.
use omnibus_db::error_ring;
use tracing_subscriber::prelude::*;

use super::ErrorRingLayer;

/// Process-wide lock: the layer writes into `omnibus_db::error_ring`'s one
/// static buffer, and installing a global tracing subscriber per test can
/// only happen once per process, so every test in this file runs serially
/// under a scoped local subscriber via `tracing::subscriber::with_default`
/// (safe to nest/repeat, unlike `try_init`) while holding this lock.
static TEST_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn with_layer<F: FnOnce()>(f: F) {
    let _guard = TEST_LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    omnibus_db::test_support::clear_error_ring();
    let subscriber = tracing_subscriber::registry().with(ErrorRingLayer);
    tracing::subscriber::with_default(subscriber, f);
}

#[test]
fn error_level_event_is_captured_into_the_ring_buffer() {
    with_layer(|| {
        tracing::error!("boom, something broke");
    });

    let snap = error_ring::snapshot();
    assert_eq!(snap.len(), 1, "one ERROR event should have been captured");
    assert_eq!(snap[0].message, "boom, something broke");
}

#[test]
fn non_error_level_events_are_not_captured() {
    with_layer(|| {
        tracing::warn!("just a warning");
        tracing::info!("just info");
        tracing::debug!("just debug");
    });

    assert!(
        error_ring::snapshot().is_empty(),
        "no non-ERROR event should have been captured"
    );
}

#[test]
fn captured_entry_carries_book_uuid_field_when_present() {
    with_layer(|| {
        tracing::error!(book_uuid = %"abc-123", "failed to process book");
    });

    let snap = error_ring::snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].book_uuid.as_deref(), Some("abc-123"));
    assert_eq!(snap[0].message, "failed to process book");
}

#[test]
fn captured_entry_carries_uuid_field_as_book_uuid_when_present() {
    // `uuid = %uuid` is the more common call-site shape in this codebase
    // (e.g. `db::covers::get_cover`) — both names must resolve to the same
    // wire field so the UI can link to `/books/{uuid}`.
    with_layer(|| {
        tracing::error!(uuid = %"def-456", "cover fetch failed");
    });

    let snap = error_ring::snapshot();
    assert_eq!(snap[0].book_uuid.as_deref(), Some("def-456"));
}

#[test]
fn captured_entry_carries_file_field_when_present() {
    with_layer(|| {
        tracing::error!(file = %"/library/book.epub", "failed to read file");
    });

    let snap = error_ring::snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].file.as_deref(), Some("/library/book.epub"));
}

#[test]
fn captured_entry_has_no_book_or_file_reference_when_event_carries_none() {
    with_layer(|| {
        tracing::error!("generic failure with no identifiers");
    });

    let snap = error_ring::snapshot();
    assert_eq!(snap[0].book_uuid, None);
    assert_eq!(snap[0].file, None);
}

#[test]
fn named_error_field_does_not_leak_secret_shaped_value_into_captured_message() {
    // `error = %e` keeps the value out of `message`, the field served to admins over HTTP.
    let secret = "sk_live_totally_secret_api_key_12345";
    with_layer(|| {
        let err = anyhow::anyhow!("{secret}");
        tracing::error!(error = %err, "operation failed");
    });

    let snap = error_ring::snapshot();
    assert_eq!(snap.len(), 1);
    assert_eq!(snap[0].message, "operation failed");
    assert!(
        !snap[0].message.contains(secret),
        "captured message must not contain the secret carried on the `error` field"
    );
}
