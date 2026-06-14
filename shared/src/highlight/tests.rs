//! Unit tests for the highlight wire-type validators.

use super::*;

fn ok_create(cfi: &str) -> CreateHighlight {
    CreateHighlight {
        book_uuid: "uuid-1".into(),
        epub_cfi_range: cfi.into(),
        color: HighlightColor::Amber,
    }
}

#[test]
fn create_highlight_validate_accepts_well_formed_payload() {
    let c = ok_create("epubcfi(/6/4!/4/2,/1:0,/1:100)");
    assert!(c.validate().is_ok());
}

#[test]
fn create_highlight_validate_accepts_cfi_at_max_len() {
    let c = ok_create(&"a".repeat(CreateHighlight::EPUB_CFI_RANGE_MAX_LEN));
    assert!(c.validate().is_ok());
}

#[test]
fn create_highlight_validate_rejects_cfi_over_max_len() {
    let c = ok_create(&"a".repeat(CreateHighlight::EPUB_CFI_RANGE_MAX_LEN + 1));
    let err = c.validate().expect_err("over-long cfi must be rejected");
    assert!(err.contains("epub_cfi_range"), "got: {err}");
    assert!(
        err.contains(&CreateHighlight::EPUB_CFI_RANGE_MAX_LEN.to_string()),
        "got: {err}"
    );
}

#[test]
fn create_highlight_validate_rejects_empty_book_uuid() {
    let mut c = ok_create("epubcfi(/6/4)");
    c.book_uuid = "   ".into();
    let err = c.validate().expect_err("blank book_uuid must be rejected");
    assert!(err.contains("book_uuid"), "got: {err}");
}

#[test]
fn create_highlight_validate_rejects_empty_cfi() {
    let c = ok_create("   ");
    let err = c
        .validate()
        .expect_err("blank epub_cfi_range must be rejected");
    assert!(err.contains("epub_cfi_range"), "got: {err}");
}

#[test]
fn update_highlight_note_validate_accepts_none() {
    let u = UpdateHighlightNote { note: None };
    assert!(u.validate().is_ok());
}

#[test]
fn update_highlight_note_validate_accepts_note_at_max_len() {
    let u = UpdateHighlightNote {
        note: Some("n".repeat(UpdateHighlightNote::NOTE_MAX_LEN)),
    };
    assert!(u.validate().is_ok());
}

#[test]
fn update_highlight_note_validate_rejects_note_over_max_len() {
    let u = UpdateHighlightNote {
        note: Some("n".repeat(UpdateHighlightNote::NOTE_MAX_LEN + 1)),
    };
    let err = u.validate().expect_err("over-long note must be rejected");
    assert!(err.contains("note"), "got: {err}");
    assert!(
        err.contains(&UpdateHighlightNote::NOTE_MAX_LEN.to_string()),
        "got: {err}"
    );
}
