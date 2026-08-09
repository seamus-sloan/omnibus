//! Validation tests for the Physical Check-In request wire types.

use super::*;
use crate::metadata_lookup::MetadataProvider;

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
        series: None,
        first_publish_year: Some(1813),
        source: MetadataProvider::OpenLibrary,
    }
}

#[test]
fn check_in_request_validate_accepts_a_well_formed_request() {
    let req = CheckInRequest {
        book_uuid: "book-1".into(),
        isbn: Some("9780141439518".into()),
        note: Some("Found at a used bookstore.".into()),
    };
    assert!(req.validate().is_ok());
}

#[test]
fn check_in_request_validate_rejects_an_empty_book_uuid() {
    let req = CheckInRequest {
        book_uuid: "   ".into(),
        isbn: None,
        note: None,
    };
    let err = req
        .validate()
        .expect_err("blank book_uuid must be rejected");
    assert!(err.contains("book_uuid"), "got: {err}");
}

#[test]
fn check_in_request_validate_rejects_an_oversized_book_uuid() {
    let req = CheckInRequest {
        book_uuid: "u".repeat(crate::BOOK_UUID_MAX_LEN + 1),
        isbn: None,
        note: None,
    };
    let err = req
        .validate()
        .expect_err("oversized book_uuid must be rejected");
    assert!(err.contains("bytes"), "got: {err}");
}

#[test]
fn check_in_request_validate_rejects_an_oversized_isbn() {
    let req = CheckInRequest {
        book_uuid: "book-1".into(),
        isbn: Some("1".repeat(ISBN_MAX_LEN + 1)),
        note: None,
    };
    let err = req.validate().expect_err("oversized isbn must be rejected");
    assert!(err.contains("isbn"), "got: {err}");
}

#[test]
fn check_in_request_validate_rejects_an_oversized_note() {
    let req = CheckInRequest {
        book_uuid: "book-1".into(),
        isbn: None,
        note: Some("x".repeat(NOTE_MAX_LEN + 1)),
    };
    let err = req.validate().expect_err("oversized note must be rejected");
    assert!(err.contains("note"), "got: {err}");
}

#[test]
fn add_physical_only_request_validate_accepts_a_well_formed_request() {
    let req = AddPhysicalOnlyRequest {
        meta: valid_meta(),
        note: Some("Gift from a friend.".into()),
    };
    assert!(req.validate().is_ok());
}

#[test]
fn add_physical_only_request_validate_rejects_an_invalid_meta() {
    let mut meta = valid_meta();
    meta.title = "x".repeat(ExternalBookMeta::TITLE_MAX_LEN + 1);
    let req = AddPhysicalOnlyRequest { meta, note: None };
    let err = req.validate().expect_err("invalid meta must be rejected");
    assert!(err.contains("title"), "got: {err}");
}

#[test]
fn add_physical_only_request_validate_rejects_an_oversized_note() {
    let req = AddPhysicalOnlyRequest {
        meta: valid_meta(),
        note: Some("x".repeat(NOTE_MAX_LEN + 1)),
    };
    let err = req.validate().expect_err("oversized note must be rejected");
    assert!(err.contains("note"), "got: {err}");
}

#[test]
fn wishlist_add_request_validate_accepts_a_book_uuid_target() {
    let req = WishlistAddRequest {
        book_uuid: Some("book-1".into()),
        meta: None,
        source: WishlistSource::Manual,
    };
    assert!(req.validate().is_ok());
}

#[test]
fn wishlist_add_request_validate_accepts_a_meta_target() {
    let req = WishlistAddRequest {
        book_uuid: None,
        meta: Some(valid_meta()),
        source: WishlistSource::Scan,
    };
    assert!(req.validate().is_ok());
}

#[test]
fn wishlist_add_request_validate_accepts_neither_target_as_a_shape_check() {
    // Whether at least one target is required is a domain rule enforced
    // downstream (`ScanError::MissingWishlistTarget`), not a shape check here.
    let req = WishlistAddRequest {
        book_uuid: None,
        meta: None,
        source: WishlistSource::Detail,
    };
    assert!(req.validate().is_ok());
}

#[test]
fn wishlist_add_request_validate_rejects_an_empty_book_uuid() {
    let req = WishlistAddRequest {
        book_uuid: Some("   ".into()),
        meta: None,
        source: WishlistSource::Manual,
    };
    let err = req
        .validate()
        .expect_err("blank book_uuid must be rejected");
    assert!(err.contains("book_uuid"), "got: {err}");
}

#[test]
fn wishlist_add_request_validate_rejects_an_oversized_book_uuid() {
    let req = WishlistAddRequest {
        book_uuid: Some("u".repeat(crate::BOOK_UUID_MAX_LEN + 1)),
        meta: None,
        source: WishlistSource::Manual,
    };
    let err = req
        .validate()
        .expect_err("oversized book_uuid must be rejected");
    assert!(err.contains("bytes"), "got: {err}");
}

#[test]
fn wishlist_add_request_validate_rejects_an_invalid_meta() {
    let mut meta = valid_meta();
    meta.title = "x".repeat(ExternalBookMeta::TITLE_MAX_LEN + 1);
    let req = WishlistAddRequest {
        book_uuid: None,
        meta: Some(meta),
        source: WishlistSource::Scan,
    };
    let err = req.validate().expect_err("invalid meta must be rejected");
    assert!(err.contains("title"), "got: {err}");
}

#[test]
fn wishlist_add_request_validate_ignores_an_invalid_meta_when_book_uuid_is_present() {
    // `book_uuid` wins over `meta` per the struct's documented precedence, so
    // a junk `meta` alongside a valid `book_uuid` must not be validated —
    // it would never be used.
    let mut meta = valid_meta();
    meta.title = "x".repeat(ExternalBookMeta::TITLE_MAX_LEN + 1);
    let req = WishlistAddRequest {
        book_uuid: Some("book-1".into()),
        meta: Some(meta),
        source: WishlistSource::Scan,
    };
    assert!(req.validate().is_ok());
}

#[test]
fn resolve_request_validate_accepts_a_well_formed_isbn() {
    let req = ResolveRequest {
        isbn: "9780141439518".into(),
    };
    assert!(req.validate().is_ok());
}

#[test]
fn resolve_request_validate_rejects_a_blank_isbn() {
    let req = ResolveRequest { isbn: "   ".into() };
    let err = req.validate().expect_err("blank isbn must be rejected");
    assert!(err.contains("isbn"), "got: {err}");
}

#[test]
fn resolve_request_validate_rejects_an_oversized_isbn() {
    let req = ResolveRequest {
        isbn: "1".repeat(ISBN_MAX_LEN + 1),
    };
    let err = req.validate().expect_err("oversized isbn must be rejected");
    assert!(err.contains("isbn"), "got: {err}");
}

#[test]
fn scan_search_request_validate_accepts_a_well_formed_query() {
    let req = ScanSearchRequest {
        query: "The Name of the Wind".into(),
    };
    assert!(req.validate().is_ok());
}

#[test]
fn scan_search_request_validate_rejects_a_blank_query() {
    let req = ScanSearchRequest {
        query: "   ".into(),
    };
    let err = req.validate().expect_err("blank query must be rejected");
    assert!(err.contains("query"), "got: {err}");
}

#[test]
fn scan_search_request_validate_rejects_an_oversized_query() {
    let req = ScanSearchRequest {
        query: "x".repeat(SEARCH_QUERY_MAX_LEN + 1),
    };
    let err = req
        .validate()
        .expect_err("oversized query must be rejected");
    assert!(err.contains("query"), "got: {err}");
}

#[test]
fn resolve_meta_request_validate_accepts_a_well_formed_meta() {
    let req = ResolveMetaRequest { meta: valid_meta() };
    assert!(req.validate().is_ok());
}

#[test]
fn resolve_meta_request_validate_rejects_an_invalid_meta() {
    let mut meta = valid_meta();
    meta.title = "x".repeat(ExternalBookMeta::TITLE_MAX_LEN + 1);
    let req = ResolveMetaRequest { meta };
    let err = req.validate().expect_err("invalid meta must be rejected");
    assert!(err.contains("title"), "got: {err}");
}

// ── ScanOutcome wire shape ─────────────────────────────────────────

fn scan_book(uuid: &str) -> ScanBook {
    ScanBook {
        uuid: uuid.into(),
        title: "Pride and Prejudice".into(),
        authors: vec!["Jane Austen".into()],
        cover_url: None,
        has_physical: false,
        isbn: None,
    }
}

#[test]
fn close_match_omits_an_empty_candidate_tail_from_the_wire() {
    // The single-candidate shape must stay byte-identical to the one that
    // shipped, or an older iOS build fails to decode a match it can act on.
    let json = serde_json::to_value(ScanOutcome::CloseMatch {
        book: scan_book("u1"),
        others: Vec::new(),
        scanned: valid_meta(),
    })
    .expect("outcome serializes");
    assert!(json.get("others").is_none(), "got: {json}");
    assert_eq!(json["kind"], "close_match");
}

#[test]
fn close_match_decodes_a_payload_that_carries_no_candidate_tail() {
    // The mirror of the above: a server that predates the picker sends no
    // `others` key at all.
    let json = serde_json::json!({
        "kind": "close_match",
        "book": serde_json::to_value(scan_book("u1")).unwrap(),
        "scanned": serde_json::to_value(valid_meta()).unwrap(),
    });
    let outcome: ScanOutcome = serde_json::from_value(json).expect("outcome decodes");
    let ScanOutcome::CloseMatch { book, others, .. } = outcome else {
        panic!("expected CloseMatch");
    };
    assert_eq!(book.uuid, "u1");
    assert!(others.is_empty());
}

#[test]
fn close_match_round_trips_every_candidate_it_was_given() {
    let outcome = ScanOutcome::CloseMatch {
        book: scan_book("epub"),
        others: vec![scan_book("audiobook")],
        scanned: valid_meta(),
    };
    let json = serde_json::to_string(&outcome).expect("outcome serializes");
    assert_eq!(
        serde_json::from_str::<ScanOutcome>(&json).expect("outcome decodes"),
        outcome
    );
}
