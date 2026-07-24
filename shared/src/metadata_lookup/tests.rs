//! Length-cap validation tests for `ExternalBookMeta`.

use super::*;

fn valid_meta() -> ExternalBookMeta {
    ExternalBookMeta {
        isbn13: "9780141439518".into(),
        title: "Pride and Prejudice".into(),
        authors: vec!["Jane Austen".into()],
        year: Some("1813".into()),
        pages: Some(432),
        publisher: Some("Penguin Classics".into()),
        description: Some("A classic novel.".into()),
        cover_url: Some("https://example.com/cover.jpg".into()),
        source: MetadataProvider::OpenLibrary,
    }
}

#[test]
fn external_book_meta_validate_accepts_a_well_formed_record() {
    assert!(valid_meta().validate().is_ok());
}

#[test]
fn external_book_meta_validate_rejects_an_oversized_title() {
    let mut meta = valid_meta();
    meta.title = "x".repeat(ExternalBookMeta::TITLE_MAX_LEN + 1);
    let err = meta
        .validate()
        .expect_err("oversized title must be rejected");
    assert!(err.contains("title"), "got: {err}");
}

#[test]
fn external_book_meta_validate_rejects_too_many_authors() {
    let mut meta = valid_meta();
    meta.authors = (0..=ExternalBookMeta::MAX_AUTHORS)
        .map(|i| format!("Author {i}"))
        .collect();
    let err = meta
        .validate()
        .expect_err("too many authors must be rejected");
    assert!(err.contains("authors"), "got: {err}");
}

#[test]
fn external_book_meta_validate_rejects_an_oversized_author_name() {
    let mut meta = valid_meta();
    meta.authors = vec!["x".repeat(ExternalBookMeta::NAME_MAX_LEN + 1)];
    let err = meta
        .validate()
        .expect_err("oversized author name must be rejected");
    assert!(err.contains("author name"), "got: {err}");
}

#[test]
fn external_book_meta_validate_rejects_an_oversized_publisher() {
    let mut meta = valid_meta();
    meta.publisher = Some("x".repeat(ExternalBookMeta::NAME_MAX_LEN + 1));
    let err = meta
        .validate()
        .expect_err("oversized publisher must be rejected");
    assert!(err.contains("publisher"), "got: {err}");
}

#[test]
fn external_book_meta_validate_rejects_an_oversized_year() {
    let mut meta = valid_meta();
    meta.year = Some("x".repeat(ExternalBookMeta::NAME_MAX_LEN + 1));
    let err = meta
        .validate()
        .expect_err("oversized year must be rejected");
    assert!(err.contains("year"), "got: {err}");
}

#[test]
fn external_book_meta_validate_rejects_an_oversized_description() {
    let mut meta = valid_meta();
    meta.description = Some("x".repeat(ExternalBookMeta::DESCRIPTION_MAX_LEN + 1));
    let err = meta
        .validate()
        .expect_err("oversized description must be rejected");
    assert!(err.contains("description"), "got: {err}");
}

#[test]
fn external_book_meta_validate_rejects_an_oversized_cover_url() {
    let mut meta = valid_meta();
    meta.cover_url = Some("x".repeat(ExternalBookMeta::COVER_URL_MAX_LEN + 1));
    let err = meta
        .validate()
        .expect_err("oversized cover_url must be rejected");
    assert!(err.contains("cover_url"), "got: {err}");
}
