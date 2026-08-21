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
        series_index: Some("1".into()),
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

// ── Hydrate request + candidate merge ────────────────────────────

fn candidate() -> ProviderEdition {
    ProviderEdition {
        source: MetadataProvider::OpenLibrary,
        provider_ref: "/works/OL1W".into(),
        isbn13: "9780141439518".into(),
        isbn10: None,
        title: "Pride and Prejudice".into(),
        authors: vec!["Jane Austen".into()],
        year: None,
        pages: Some(432),
        publisher: None,
        description: None,
        cover_url: Some("https://covers.openlibrary.org/b/id/1-L.jpg".into()),
        series: None,
        series_index: None,
        first_publish_year: Some(1813),
        genres: vec!["Fiction".into()],
    }
}

fn hydrate_request() -> EditionHydrateRequest {
    EditionHydrateRequest {
        source: MetadataProvider::OpenLibrary,
        provider_ref: "/works/OL1W".into(),
        isbn13: "9780141439518".into(),
    }
}

#[test]
fn edition_hydrate_request_validate_accepts_a_well_formed_body() {
    assert!(hydrate_request().validate().is_ok());
}

#[test]
fn edition_hydrate_request_validate_rejects_a_blank_provider_ref() {
    let mut req = hydrate_request();
    req.provider_ref = "   ".into();
    assert_eq!(req.validate(), Err("provider_ref is required".to_string()));
}

#[test]
fn edition_hydrate_request_validate_rejects_an_oversized_provider_ref() {
    let mut req = hydrate_request();
    req.provider_ref = "x".repeat(EditionHydrateRequest::PROVIDER_REF_MAX_LEN + 1);
    assert!(req
        .validate()
        .is_err_and(|m| m.starts_with("provider_ref exceeds")));
}

#[test]
fn edition_hydrate_request_validate_rejects_a_blank_isbn13() {
    let mut req = hydrate_request();
    req.isbn13 = String::new();
    assert_eq!(req.validate(), Err("isbn13 is required".to_string()));
}

#[test]
fn fill_missing_from_takes_only_the_fields_the_detail_record_lacks() {
    // The detail record's shape: publisher and the printing's own year, but
    // no work-level subjects or first-publish year.
    let mut detail = ProviderEdition {
        title: "Pride and Prejudice: Penguin Classics".into(),
        year: Some("2003".into()),
        publisher: Some("Penguin".into()),
        pages: Some(480),
        first_publish_year: None,
        genres: Vec::new(),
        cover_url: None,
        ..candidate()
    };
    detail.fill_missing_from(&candidate());

    // Held: every field the detail record answered for.
    assert_eq!(detail.title, "Pride and Prejudice: Penguin Classics");
    assert_eq!(detail.year.as_deref(), Some("2003"));
    assert_eq!(detail.pages, Some(480));
    // Filled: the fields only the search hit had.
    assert_eq!(detail.first_publish_year, Some(1813));
    assert_eq!(detail.genres, vec!["Fiction".to_string()]);
    assert_eq!(
        detail.cover_url.as_deref(),
        Some("https://covers.openlibrary.org/b/id/1-L.jpg")
    );
}

#[test]
fn fill_missing_from_treats_a_whitespace_only_value_as_absent() {
    let mut detail = ProviderEdition {
        publisher: Some("   ".into()),
        ..candidate()
    };
    let richer = ProviderEdition {
        publisher: Some("Penguin".into()),
        ..candidate()
    };
    detail.fill_missing_from(&richer);
    assert_eq!(detail.publisher.as_deref(), Some("Penguin"));
}

#[test]
fn fill_missing_from_never_blanks_a_value_the_thinner_record_lacks() {
    let mut detail = ProviderEdition {
        description: Some("A detail-record description.".into()),
        ..candidate()
    };
    // The search hit has no description at all — the merge must not reach
    // into `detail` and clear one.
    detail.fill_missing_from(&candidate());
    assert_eq!(
        detail.description.as_deref(),
        Some("A detail-record description.")
    );
}

#[test]
fn fill_missing_from_does_not_fill_an_absent_field_with_a_blank_one() {
    // Providers don't consistently trim these — Hardcover passes its image
    // URL straight through — and filling `None` with `Some("   ")` turns
    // "this source didn't say" into a present-but-empty value downstream.
    let mut detail = ProviderEdition {
        publisher: None,
        cover_url: None,
        ..candidate()
    };
    let blank = ProviderEdition {
        publisher: Some("   ".into()),
        cover_url: Some(" ".into()),
        ..candidate()
    };
    detail.fill_missing_from(&blank);
    assert_eq!(detail.publisher, None);
    assert_eq!(detail.cover_url, None);
}
