//! Unit tests for the bookmark wire-type validators.

use super::*;

fn ok_create(position: &str) -> CreateBookmark {
    CreateBookmark {
        book_uuid: "uuid-1".into(),
        position: position.into(),
        title: Some("A mark".into()),
    }
}

#[test]
fn create_bookmark_validate_accepts_well_formed_payload() {
    assert!(ok_create("1234.5").validate().is_ok());
    assert!(ok_create("epubcfi(/6/4!/4/2)").validate().is_ok());
}

#[test]
fn create_bookmark_validate_accepts_none_title() {
    let mut c = ok_create("12.0");
    c.title = None;
    assert!(c.validate().is_ok());
}

#[test]
fn create_bookmark_validate_accepts_position_and_title_at_max_len() {
    let mut c = ok_create(&"a".repeat(CreateBookmark::POSITION_MAX_LEN));
    c.title = Some("t".repeat(CreateBookmark::TITLE_MAX_LEN));
    assert!(c.validate().is_ok());
}

#[test]
fn create_bookmark_validate_rejects_empty_book_uuid() {
    let mut c = ok_create("12.0");
    c.book_uuid = "   ".into();
    let err = c.validate().expect_err("blank book_uuid must be rejected");
    assert!(err.contains("book_uuid"), "got: {err}");
}

#[test]
fn create_bookmark_validate_rejects_empty_position() {
    let c = ok_create("   ");
    let err = c.validate().expect_err("blank position must be rejected");
    assert!(err.contains("position"), "got: {err}");
}

#[test]
fn create_bookmark_validate_rejects_position_over_max_len() {
    let c = ok_create(&"a".repeat(CreateBookmark::POSITION_MAX_LEN + 1));
    let err = c
        .validate()
        .expect_err("over-long position must be rejected");
    assert!(err.contains("position"), "got: {err}");
    assert!(
        err.contains(&CreateBookmark::POSITION_MAX_LEN.to_string()),
        "got: {err}"
    );
}

#[test]
fn create_bookmark_validate_rejects_title_over_max_len() {
    let mut c = ok_create("12.0");
    c.title = Some("t".repeat(CreateBookmark::TITLE_MAX_LEN + 1));
    let err = c.validate().expect_err("over-long title must be rejected");
    assert!(err.contains("title"), "got: {err}");
}

#[test]
fn update_bookmark_validate_accepts_none() {
    assert!(UpdateBookmark { title: None }.validate().is_ok());
}

#[test]
fn update_bookmark_validate_accepts_title_at_max_len() {
    let u = UpdateBookmark {
        title: Some("t".repeat(UpdateBookmark::TITLE_MAX_LEN)),
    };
    assert!(u.validate().is_ok());
}

#[test]
fn update_bookmark_validate_rejects_title_over_max_len() {
    let u = UpdateBookmark {
        title: Some("t".repeat(UpdateBookmark::TITLE_MAX_LEN + 1)),
    };
    let err = u.validate().expect_err("over-long title must be rejected");
    assert!(err.contains("title"), "got: {err}");
}
