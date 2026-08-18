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
        series: Some("Penguin English Library".into()),
        first_publish_year: Some(1813),
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
fn external_book_meta_validate_rejects_an_oversized_series() {
    let mut meta = valid_meta();
    meta.series = Some("x".repeat(ExternalBookMeta::NAME_MAX_LEN + 1));
    let err = meta
        .validate()
        .expect_err("oversized series must be rejected");
    assert!(err.contains("series"), "got: {err}");
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
    assert!(err.contains("bytes"), "got: {err}");
}

#[test]
fn external_book_meta_validate_rejects_a_cover_url_over_the_byte_cap_even_with_few_chars() {
    // `COVER_URL_MAX_LEN` is a byte cap: a 2-byte-per-char string can exceed
    // it in bytes while staying under it in `chars().count()`, so validation
    // must measure bytes here (unlike every other field on this type).
    let mut meta = valid_meta();
    let two_byte_char = "é";
    let char_count = (ExternalBookMeta::COVER_URL_MAX_LEN / 2) + 1;
    assert!(char_count < ExternalBookMeta::COVER_URL_MAX_LEN);
    meta.cover_url = Some(two_byte_char.repeat(char_count));
    let err = meta
        .validate()
        .expect_err("a cover_url over the byte cap must be rejected");
    assert!(err.contains("cover_url"), "got: {err}");
}

// ── fan-out edition search ───────────────────────────────────────

fn edition() -> ProviderEdition {
    ProviderEdition::from_meta(valid_meta(), "9780141439518".into())
}

#[test]
fn provider_edition_from_meta_carries_the_source_and_ref() {
    let e = edition();
    assert_eq!(e.source, MetadataProvider::OpenLibrary);
    assert_eq!(e.provider_ref, "9780141439518");
    assert_eq!(e.title, "Pride and Prejudice");
}

#[test]
fn external_book_meta_from_provider_edition_round_trips_every_field() {
    let meta = valid_meta();
    let back: ExternalBookMeta = ProviderEdition::from_meta(meta.clone(), "ref".into()).into();
    assert_eq!(back, meta);
}

#[test]
fn edition_search_request_validate_accepts_a_plain_query() {
    let req = EditionSearchRequest {
        query: "dune".into(),
        providers: None,
    };
    assert!(req.validate().is_ok());
}

#[test]
fn edition_search_request_validate_rejects_a_blank_query() {
    let req = EditionSearchRequest {
        query: "   ".into(),
        providers: None,
    };
    assert!(req
        .validate()
        .expect_err("blank must fail")
        .contains("query"));
}

#[test]
fn edition_search_request_validate_rejects_an_oversized_query() {
    let req = EditionSearchRequest {
        query: "x".repeat(EDITION_SEARCH_QUERY_MAX_LEN + 1),
        providers: None,
    };
    let err = req.validate().expect_err("oversized must fail");
    assert!(err.contains("exceeds"), "got: {err}");
}

#[test]
fn edition_search_request_validate_rejects_an_oversized_provider_filter() {
    let req = EditionSearchRequest {
        query: "dune".into(),
        providers: Some(vec![
            MetadataProvider::OpenLibrary;
            EDITION_SEARCH_MAX_PROVIDERS + 1
        ]),
    };
    let err = req.validate().expect_err("oversized filter must fail");
    assert!(err.contains("providers"), "got: {err}");
}

#[test]
fn provider_search_status_serializes_as_a_tagged_status_field() {
    let answered = serde_json::to_value(ProviderSearchStatus::Answered { count: 2 }).unwrap();
    assert_eq!(answered["status"], "answered");
    assert_eq!(answered["count"], 2);
    let missing = serde_json::to_value(ProviderSearchStatus::NotConfigured).unwrap();
    assert_eq!(missing["status"], "not_configured");
    let failed = serde_json::to_value(ProviderSearchStatus::Failed {
        message: "timed out".into(),
    })
    .unwrap();
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["message"], "timed out");
}

#[test]
fn edition_search_response_flattens_each_source_status_onto_its_row() {
    let response = EditionSearchResponse {
        editions: vec![edition()],
        sources: vec![ProviderSourceStatus {
            provider: MetadataProvider::OpenLibrary,
            display_name: MetadataProvider::OpenLibrary.display_name().into(),
            status: ProviderSearchStatus::Answered { count: 1 },
        }],
    };
    let json = serde_json::to_value(&response).unwrap();
    assert_eq!(json["sources"][0]["provider"], "open_library");
    assert_eq!(json["sources"][0]["status"], "answered");
    assert_eq!(json["sources"][0]["count"], 1);
    assert_eq!(json["editions"][0]["source"], "open_library");
    let back: EditionSearchResponse = serde_json::from_value(json).unwrap();
    assert_eq!(back, response);
}
