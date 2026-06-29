//! Validation tests for the journal wire types.

use super::*;

fn create(body: &str) -> CreateJournalEntry {
    CreateJournalEntry {
        book_uuid: "book-1".into(),
        body_md: body.into(),
        progress: None,
    }
}

#[test]
fn create_accepts_a_normal_entry() {
    assert!(create("Loved the ending.").validate().is_ok());
}

#[test]
fn create_rejects_empty_uuid() {
    let mut c = create("text");
    c.book_uuid = "   ".into();
    let err = c.validate().expect_err("blank uuid must be rejected");
    assert!(err.contains("book_uuid"), "got: {err}");
}

#[test]
fn create_rejects_blank_body() {
    let err = create("   \n\t ")
        .validate()
        .expect_err("whitespace-only body must be rejected");
    assert!(err.contains("empty"), "got: {err}");
}

#[test]
fn create_rejects_oversized_body() {
    let mut c = create("");
    c.body_md = "a".repeat(BODY_MAX_LEN + 1);
    let err = c.validate().expect_err("oversized body must be rejected");
    assert!(err.contains("bytes"), "got: {err}");
}

#[test]
fn create_accepts_body_at_max_len() {
    let mut c = create("");
    c.body_md = "a".repeat(BODY_MAX_LEN);
    assert!(c.validate().is_ok());
}

#[test]
fn create_rejects_progress_above_max() {
    let mut c = create("text");
    c.progress = Some(PROGRESS_MAX + 1);
    let err = c.validate().expect_err("progress > 100 must be rejected");
    assert!(err.contains("progress"), "got: {err}");
}

#[test]
fn create_accepts_progress_at_bounds() {
    for p in [0u8, 50, PROGRESS_MAX] {
        let mut c = create("text");
        c.progress = Some(p);
        assert!(c.validate().is_ok(), "progress={p} should be valid");
    }
}

#[test]
fn update_validates_body_and_progress() {
    assert!(UpdateJournalEntry {
        body_md: "ok".into(),
        progress: Some(100)
    }
    .validate()
    .is_ok());
    assert!(UpdateJournalEntry {
        body_md: " ".into(),
        progress: None
    }
    .validate()
    .is_err());
}
