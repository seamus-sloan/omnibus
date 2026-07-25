//! Tests for the [`super`] error sanitizers: every branch that can carry a
//! foreign-system internal (SMTP/lettre, subprocess stderr, a DB driver)
//! must never let that text reach the returned [`TaskOutcome::Err`].

use super::{anyhow_outcome, kepub_outcome, kindle_outcome, sanitized_err};
use crate::kepub::KepubError;
use crate::kindle::KindleError;
use crate::worker::TaskOutcome;

fn err_text(outcome: TaskOutcome) -> String {
    match outcome {
        TaskOutcome::Err(msg) => msg,
        TaskOutcome::Ok(_) => panic!("expected TaskOutcome::Err"),
    }
}

#[test]
fn sanitized_err_names_the_label_and_drops_the_raw_text() {
    let outcome = sanitized_err("thumbnail generation", "raw internal: /etc/secret-path");
    let msg = err_text(outcome);
    assert!(msg.contains("thumbnail generation"));
    assert!(!msg.contains("/etc/secret-path"));
}

#[test]
fn anyhow_outcome_returns_ok_none_on_success() {
    let outcome = anyhow_outcome::<()>("library scan", Ok(()));
    assert!(matches!(outcome, TaskOutcome::Ok(None)));
}

#[test]
fn anyhow_outcome_sanitizes_the_anyhow_error_text() {
    let outcome = anyhow_outcome::<()>(
        "library scan",
        Err(anyhow::anyhow!(
            "scan of /srv/library failed: permission denied"
        )),
    );
    let msg = err_text(outcome);
    assert!(msg.contains("library scan"));
    assert!(!msg.contains("/srv/library"));
    assert!(!msg.contains("permission denied"));
}

#[test]
fn kindle_outcome_returns_ok_none_on_success() {
    assert!(matches!(kindle_outcome(Ok(())), TaskOutcome::Ok(None)));
}

#[test]
fn kindle_outcome_passes_through_the_safe_enumerable_variants() {
    for (err, expected) in [
        (
            KindleError::NotConfigured,
            "email delivery is not configured on this server",
        ),
        (KindleError::NoEpub, "this book has no EPUB file to send"),
    ] {
        let msg = err_text(kindle_outcome(Err(err)));
        assert_eq!(msg, expected);
    }
}

#[test]
fn kindle_outcome_sanitizes_the_lettre_transport_variants() {
    let msg = err_text(kindle_outcome(Err(KindleError::Address(
        lettre::address::AddressError::MissingParts,
    ))));
    assert!(!msg.contains("MissingParts"));
    assert!(msg.contains("send to Kindle"));
}

#[test]
fn kindle_outcome_sanitizes_io_errors() {
    let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "/home/alice/secret.epub");
    let msg = err_text(kindle_outcome(Err(KindleError::Io(io_err))));
    assert!(!msg.contains("/home/alice/secret.epub"));
    assert!(msg.contains("send to Kindle"));
}

#[test]
fn kepub_outcome_returns_ok_none_on_success() {
    assert!(matches!(
        kepub_outcome(Ok(std::path::PathBuf::from("/tmp/book.kepub.epub"))),
        TaskOutcome::Ok(None)
    ));
}

#[test]
fn kepub_outcome_passes_through_the_safe_book_id_variants() {
    let msg = err_text(kepub_outcome(Err(KepubError::BookNotFound(42))));
    assert_eq!(msg, "book 42 not found");
}

#[test]
fn kepub_outcome_sanitizes_subprocess_stderr() {
    let msg = err_text(kepub_outcome(Err(KepubError::Failed(anyhow::anyhow!(
        "kepubify exited with exit status: 1: panic: invalid EPUB structure at offset 0xdeadbeef"
    )))));
    assert!(!msg.contains("0xdeadbeef"));
    assert!(msg.contains("kepub conversion"));
}
