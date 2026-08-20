//! Length-cap validation tests for `ExternalBookMeta`, plus the fan-out
//! search's request validation and its narrowing conversion.

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
    ProviderEdition {
        source: MetadataProvider::GoogleBooks,
        provider_ref: "gb-volume-1".into(),
        isbn13: "9780141439518".into(),
        isbn10: Some("0141439513".into()),
        title: "Pride and Prejudice".into(),
        authors: vec!["Jane Austen".into()],
        year: Some("1813".into()),
        pages: Some(432),
        publisher: Some("Penguin Classics".into()),
        description: Some("A classic novel.".into()),
        cover_url: Some("https://example.com/cover.jpg".into()),
        series: Some("Penguin English Library".into()),
        first_publish_year: Some(1813),
        genres: vec!["Fiction".into(), "Classics".into()],
    }
}

#[test]
fn provider_edition_narrows_to_external_book_meta_carrying_every_shared_field() {
    let meta = ExternalBookMeta::from(edition());
    // The check-in payload is the same record — attribution included — minus
    // the handle only the picker needs.
    assert_eq!(
        meta,
        ExternalBookMeta {
            source: MetadataProvider::GoogleBooks,
            ..valid_meta()
        }
    );
}

#[test]
fn provider_edition_deserializes_without_the_picker_only_fields() {
    // Both are `#[serde(default)]`, so a candidate serialized before they
    // existed — or by a provider that carries neither — still parses.
    let json = serde_json::json!({
        "source": "google_books",
        "provider_ref": "gb-volume-1",
        "isbn13": "9780141439518",
        "title": "Pride and Prejudice",
        "authors": ["Jane Austen"],
        "year": null,
        "pages": null,
        "publisher": null,
        "description": null,
        "cover_url": null,
        "series": null,
        "first_publish_year": null,
    });
    let parsed: ProviderEdition =
        serde_json::from_value(json).expect("the new fields must be optional on the wire");
    assert_eq!(parsed.isbn10, None);
    assert!(parsed.genres.is_empty());
}

#[test]
fn edition_search_request_validate_accepts_a_query_with_no_provider_filter() {
    let req = EditionSearchRequest {
        query: "pride and prejudice".into(),
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
    let err = req.validate().expect_err("a blank query must be rejected");
    assert!(err.contains("query is required"), "got: {err}");
}

#[test]
fn edition_search_request_validate_rejects_an_oversized_query() {
    let req = EditionSearchRequest {
        query: "x".repeat(SEARCH_QUERY_MAX_LEN + 1),
        providers: None,
    };
    let err = req
        .validate()
        .expect_err("an oversized query must be rejected");
    assert!(err.contains("exceeds"), "got: {err}");
}

#[test]
fn edition_search_request_validate_rejects_an_explicitly_empty_provider_list() {
    let req = EditionSearchRequest {
        query: "pride and prejudice".into(),
        providers: Some(Vec::new()),
    };
    let err = req
        .validate()
        .expect_err("an empty provider list must be rejected");
    assert!(err.contains("at least one provider"), "got: {err}");
}

#[test]
fn provider_search_status_serializes_its_three_cases_distinguishably() {
    // A client has to tell these apart to say "couldn't reach it" rather than
    // "nothing found", so the tag is part of the wire contract.
    let json = |s: &ProviderSearchStatus| serde_json::to_value(s).unwrap();
    assert_eq!(
        json(&ProviderSearchStatus::Answered { count: 2 }),
        serde_json::json!({ "kind": "answered", "count": 2 })
    );
    assert_eq!(
        json(&ProviderSearchStatus::NotConfigured),
        serde_json::json!({ "kind": "not_configured" })
    );
    assert_eq!(
        json(&ProviderSearchStatus::Failed {
            message: "open library returned an error status".into()
        }),
        serde_json::json!({
            "kind": "failed",
            "message": "open library returned an error status"
        })
    );
}

#[test]
fn provider_as_str_matches_the_serde_tag_it_is_stored_alongside() {
    // `book_external_ratings.provider` stores `as_str` while every wire
    // payload carries the serde tag; a drift between them would orphan rows.
    for provider in MetadataProvider::ALL {
        assert_eq!(
            serde_json::to_value(provider).unwrap(),
            serde_json::Value::String(provider.as_str().to_string()),
            "{provider:?} serializes differently from its stored token"
        );
    }
}

#[test]
fn provider_from_str_round_trips_every_variant() {
    for provider in MetadataProvider::ALL {
        assert_eq!(
            MetadataProvider::from_str(provider.as_str()),
            Some(provider)
        );
    }
}

#[test]
fn provider_from_str_returns_none_for_an_unknown_token() {
    // A row written by a build that knew a source this one doesn't.
    assert_eq!(MetadataProvider::from_str("goodreads"), None);
}
