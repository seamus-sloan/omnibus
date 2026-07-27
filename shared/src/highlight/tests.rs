//! Unit tests for the highlight wire-type validators.

use super::*;

fn ok_create(cfi: &str) -> CreateHighlight {
    CreateHighlight {
        client_id: None,
        book_uuid: "uuid-1".into(),
        epub_cfi_range: cfi.into(),
        color: HighlightColor::Amber,
        text: None,
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
fn create_highlight_validate_accepts_text_at_max_len() {
    let mut c = ok_create("epubcfi(/6/4)");
    c.text = Some("t".repeat(CreateHighlight::TEXT_MAX_LEN));
    assert!(c.validate().is_ok());
}

#[test]
fn create_highlight_validate_rejects_text_over_max_len() {
    let mut c = ok_create("epubcfi(/6/4)");
    c.text = Some("t".repeat(CreateHighlight::TEXT_MAX_LEN + 1));
    let err = c.validate().expect_err("over-long text must be rejected");
    assert!(err.contains("text"), "got: {err}");
}

#[test]
fn highlight_decodes_a_payload_with_null_or_missing_cfi_range() {
    // Kobo-origin rows serve `epub_cfi_range: null`; older stored payloads
    // may omit the key entirely. Both must decode to `None`.
    let with_null: Highlight = serde_json::from_str(
        r#"{"id":1,"book_uuid":"u","epub_cfi_range":null,"color":"amber","note":null,"created_at":5}"#,
    )
    .expect("null cfi must decode");
    assert_eq!(with_null.epub_cfi_range, None);

    let missing: Highlight = serde_json::from_str(
        r#"{"id":1,"book_uuid":"u","color":"amber","note":null,"created_at":5}"#,
    )
    .expect("missing cfi must decode");
    assert_eq!(missing.epub_cfi_range, None);
}

#[test]
fn highlight_serializes_an_anchorless_row_with_an_explicit_null_cfi() {
    let h = Highlight {
        id: 7,
        book_uuid: "u".into(),
        epub_cfi_range: None,
        color: HighlightColor::Rose,
        note: None,
        text: Some("quote".into()),
        client_id: Some("kobo-uuid".into()),
        created_at: 5,
    };
    let json = serde_json::to_value(&h).expect("serialize");
    // The key stays present (honest null) so decoders see a stable shape.
    assert!(json.get("epub_cfi_range").is_some());
    assert!(json["epub_cfi_range"].is_null());
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
