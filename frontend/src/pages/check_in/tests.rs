use super::screens::byline;
use super::*;
use omnibus_shared::MetadataProvider;

fn scan_book() -> ScanBook {
    ScanBook {
        uuid: "book-uuid".into(),
        title: "Dune".into(),
        authors: vec!["Frank Herbert".into()],
        cover_url: None,
        has_physical: false,
        isbn: Some("9780441013593".into()),
    }
}

fn external() -> ExternalBookMeta {
    ExternalBookMeta {
        isbn13: "9780441013593".into(),
        title: "Dune".into(),
        authors: vec!["Frank Herbert".into()],
        year: Some("1965".into()),
        pages: None,
        publisher: None,
        description: None,
        cover_url: None,
        source: MetadataProvider::OpenLibrary,
    }
}

#[test]
fn clean_isbn_strips_separators_and_upcases_the_check_digit() {
    assert_eq!(clean_isbn("978-0-441-01359-3"), "9780441013593");
    assert_eq!(clean_isbn(" 0 441 17271 x "), "044117271X");
}

#[test]
fn looks_like_isbn_accepts_ten_and_thirteen_digit_forms() {
    assert!(looks_like_isbn("9780441013593"));
    assert!(looks_like_isbn("0441172717"));
    assert!(looks_like_isbn("044117271X"));
}

#[test]
fn looks_like_isbn_rejects_wrong_length_or_stray_letters() {
    assert!(!looks_like_isbn(""));
    assert!(!looks_like_isbn("97804410135"));
    assert!(!looks_like_isbn("97804410135931"));
    assert!(!looks_like_isbn("978044101359X"), "X is ISBN-10 only");
    assert!(!looks_like_isbn("044117271A"));
    assert!(!looks_like_isbn("04411X2717"), "X is the check digit only");
}

#[test]
fn stage_for_already_owned_yields_no_stage_so_the_caller_navigates() {
    let outcome = ScanOutcome::AlreadyOwned { book: scan_book() };
    assert!(stage_for(outcome, "9780441013593").is_none());
}

#[test]
fn stage_for_in_library_unowned_opens_the_check_in_confirm() {
    let outcome = ScanOutcome::InLibraryUnowned { book: scan_book() };
    let Some(Stage::Confirm { book, isbn }) = stage_for(outcome, "9780441013593") else {
        panic!("expected the confirm stage");
    };
    assert_eq!(book.uuid, "book-uuid");
    assert_eq!(isbn, "9780441013593");
}

#[test]
fn stage_for_close_match_opens_the_confirm_prompt_not_a_write() {
    let outcome = ScanOutcome::CloseMatch {
        book: scan_book(),
        scanned: external(),
    };
    assert!(matches!(
        stage_for(outcome, "9780441013593"),
        Some(Stage::CloseMatch { .. })
    ));
}

#[test]
fn stage_for_not_in_library_opens_the_own_or_wishlist_chooser() {
    let outcome = ScanOutcome::NotInLibrary { online: external() };
    assert!(matches!(
        stage_for(outcome, "9780441013593"),
        Some(Stage::Choose { .. })
    ));
}

#[test]
fn stage_for_unresolved_keeps_the_isbn_for_the_not_found_message() {
    let Some(Stage::Unresolved { isbn }) = stage_for(ScanOutcome::Unresolved, "9780441013593")
    else {
        panic!("expected the unresolved stage");
    };
    assert_eq!(isbn, "9780441013593");
}

#[test]
fn some_if_filled_drops_blank_notes() {
    assert_eq!(some_if_filled("  "), None);
    assert_eq!(
        some_if_filled(" first printing "),
        Some("first printing".into())
    );
}

#[test]
fn wishlist_request_for_targets_the_online_meta_and_records_the_scan_source() {
    let req = wishlist_request_for(&external());
    assert!(req.book_uuid.is_none());
    assert_eq!(req.meta.map(|m| m.isbn13), Some("9780441013593".into()));
    assert_eq!(req.source, WishlistSource::Scan);
}

#[test]
fn friendly_error_strips_the_server_function_framing() {
    assert_eq!(
        friendly_error(
            "error running server function: ISBN has an invalid check digit (details: None)"
        ),
        "ISBN has an invalid check digit"
    );
}

#[test]
fn friendly_error_passes_a_plain_message_through() {
    assert_eq!(
        friendly_error("network error: timed out"),
        "network error: timed out"
    );
}

#[test]
fn friendly_error_keeps_the_original_when_stripping_leaves_nothing() {
    // A bare framing with no message of its own is still better than "".
    assert_eq!(
        friendly_error("error running server function:(details: None)"),
        "error running server function:(details: None)"
    );
}

#[test]
fn byline_joins_names_with_a_trailing_and() {
    assert_eq!(byline(&[]), "");
    assert_eq!(byline(&["A".into()]), "A");
    assert_eq!(byline(&["A".into(), "B".into()]), "A and B");
    assert_eq!(byline(&["A".into(), "B".into(), "C".into()]), "A, B and C");
    assert_eq!(byline(&["A".into(), " ".into()]), "A");
}
