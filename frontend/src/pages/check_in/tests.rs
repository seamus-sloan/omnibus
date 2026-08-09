//! Tests for the check-in flow's pure helpers: ISBN cleaning and check-digit
//! rejection, keypad entry, front-door and failure-fallback stage choice,
//! wizard stage selection per lookup outcome, the byline/notes formatting used
//! by the confirm screens, and the shared jump-to-title-search handler.

use super::entry::apply_key;
use super::link::{scan_book_from_hit, truncation_note};
use super::screens::{byline, meta_details, CloseMatchCopy};
use super::*;
use omnibus_shared::{MetadataProvider, PaletteBookHit};

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
        series: None,
        first_publish_year: None,
        source: MetadataProvider::OpenLibrary,
    }
}

#[test]
fn clean_isbn_strips_separators_and_upcases_the_check_digit() {
    assert_eq!(clean_isbn("978-0-441-01359-3"), "9780441013593");
    assert_eq!(clean_isbn(" 0 441 17271 x "), "044117271X");
}

#[test]
fn isbn_rejection_accepts_valid_ten_and_thirteen_digit_forms() {
    assert_eq!(isbn_rejection("9780441013593"), None);
    assert_eq!(isbn_rejection("0441172717"), None);
    // 123456789X is a valid ISBN-10 whose check digit is X.
    assert_eq!(isbn_rejection("123456789X"), None);
}

#[test]
fn isbn_rejection_reports_a_plain_length_message_for_a_partial_entry() {
    let msg = Some("Enter a 10- or 13-digit ISBN.".to_string());
    assert_eq!(isbn_rejection(""), msg);
    assert_eq!(isbn_rejection("97804410135"), msg);
    assert_eq!(isbn_rejection("97804410135931"), msg);
}

#[test]
fn isbn_rejection_catches_a_transposed_digit_before_the_round_trip() {
    // Right length, wrong check digit — the case a shape-only gate misses.
    assert_eq!(
        isbn_rejection("9780441013539").as_deref(),
        Some("ISBN has an invalid check digit")
    );
    assert_eq!(
        isbn_rejection("0441172718").as_deref(),
        Some("ISBN has an invalid check digit")
    );
}

#[test]
fn isbn_rejection_catches_stray_letters() {
    assert_eq!(
        isbn_rejection("978044101359X").as_deref(),
        Some("ISBN contains invalid characters"),
        "X is ISBN-10 only"
    );
    assert_eq!(
        isbn_rejection("04411X2717").as_deref(),
        Some("ISBN contains invalid characters"),
        "X is the check digit only"
    );
}

#[test]
fn apply_key_appends_up_to_thirteen_slots_and_backspaces() {
    assert_eq!(apply_key("978", "0"), "9780");
    assert_eq!(apply_key("978-0", "4"), "97804");
    assert_eq!(apply_key("9780", "\u{232b}"), "978");
    assert_eq!(apply_key("", "\u{232b}"), "");
    // The 14th press is dropped rather than silently truncating the entry.
    assert_eq!(apply_key("9780441013593", "7"), "9780441013593");
}

/// Web and SSR open on the fields, so raising check-in never starts a camera.
#[cfg(not(feature = "mobile"))]
#[test]
fn front_door_opens_on_the_lookup_fields_rather_than_the_camera() {
    assert!(matches!(front_door(), Stage::Lookup));
}

/// The mobile shell keeps the camera as its front door.
#[cfg(feature = "mobile")]
#[test]
fn front_door_opens_on_the_camera_for_the_mobile_shell() {
    assert!(matches!(front_door(), Stage::Scan));
}

#[test]
fn manual_fallback_keeps_a_rejected_lookup_on_the_screen_it_was_typed_on() {
    assert!(matches!(manual_fallback(&Stage::Lookup), Stage::Lookup));
}

#[test]
fn manual_fallback_sends_a_bad_camera_decode_to_the_keypad() {
    assert!(matches!(manual_fallback(&Stage::Scan), Stage::Entry));
    assert!(matches!(manual_fallback(&Stage::Entry), Stage::Entry));
}

#[test]
fn stage_for_already_owned_yields_no_stage_so_the_caller_navigates() {
    let outcome = ScanOutcome::AlreadyOwned { book: scan_book() };
    assert!(stage_for(outcome, "9780441013593").is_none());
}

#[test]
fn stage_for_on_wishlist_yields_no_stage_so_the_caller_navigates() {
    let outcome = ScanOutcome::OnWishlist { book: scan_book() };
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
        others: Vec::new(),
        scanned: external(),
    };
    let Some(Stage::CloseMatch { books, .. }) = stage_for(outcome, "9780441013593") else {
        panic!("expected the close-match stage");
    };
    assert_eq!(books.len(), 1);
}

#[test]
fn stage_for_close_match_flattens_the_wire_head_and_tail_into_one_picker() {
    // Two rows for one work (an EPUB and its unattached audiobook) arrive as
    // `book` + `others`; the screen renders them as a single ordered list.
    let second = ScanBook {
        uuid: "audiobook-uuid".into(),
        ..scan_book()
    };
    let outcome = ScanOutcome::CloseMatch {
        book: scan_book(),
        others: vec![second],
        scanned: external(),
    };
    let Some(Stage::CloseMatch { books, .. }) = stage_for(outcome, "9780441013593") else {
        panic!("expected the close-match stage");
    };
    assert_eq!(
        books.iter().map(|b| b.uuid.as_str()).collect::<Vec<_>>(),
        vec!["book-uuid", "audiobook-uuid"]
    );
}

#[test]
fn close_match_copy_asks_which_one_only_when_several_books_are_offered() {
    let one = CloseMatchCopy::for_count(false);
    assert_eq!(one.heading, "Is this the book?");
    assert_eq!(one.pick, "Yes, that's it");
    assert_eq!(one.decline, "No, different book");

    let many = CloseMatchCopy::for_count(true);
    assert_eq!(many.heading, "Which one is this?");
    assert_eq!(many.pick, "This one");
    assert_eq!(many.decline, "None of these");
    // A picker that says "match this one" reads as a single suggestion.
    assert!(many.subtitle.ends_with("match these."), "{}", many.subtitle);
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
fn meta_details_joins_the_facts_the_provider_carried() {
    let mut meta = external();
    meta.first_publish_year = Some(1965);
    meta.publisher = Some("Chilton Books".into());
    meta.pages = Some(412);
    assert_eq!(
        meta_details(&meta).as_deref(),
        Some("First published 1965 \u{b7} Chilton Books \u{b7} 412 pages")
    );
}

#[test]
fn meta_details_is_none_when_the_provider_carried_nothing() {
    // `external()` has no first-publish year, publisher, or page count.
    assert_eq!(meta_details(&external()), None);
    // A blank publisher and a zero page count don't count as facts either.
    let mut meta = external();
    meta.publisher = Some("   ".into());
    meta.pages = Some(0);
    assert_eq!(meta_details(&meta), None);
}

// `Signal::new` needs a live Dioxus runtime, so the handler runs inside a
// throwaway component driven by a bare `VirtualDom` rebuild — the same idiom
// `contexts/tests.rs` uses to exercise signal-mutating APIs from a `#[test]`.
#[test]
fn go_to_search_opens_the_lookup_stage_and_clears_a_stale_error() {
    #[component]
    fn AssertGoToSearch() -> Element {
        let state = FlowState {
            stage: Signal::new(Stage::Entry),
            isbn: Signal::new("978044101359".to_string()),
            note: Signal::new(String::new()),
            busy: Signal::new(false),
            error: Signal::new(Some("Could not look that up".to_string())),
        };
        go_to_search(state).call(());

        assert!(matches!((state.stage)(), Stage::Lookup));
        // A failed lookup's message must not follow the reader onto a screen
        // that no longer describes it.
        assert_eq!((state.error)(), None);
        // The typed ISBN survives, so "Scan a barcode instead" is the only
        // thing that discards it.
        assert_eq!((state.isbn)(), "978044101359");

        rsx! {}
    }
    VirtualDom::new(AssertGoToSearch).rebuild_in_place();
}

fn palette_hit() -> PaletteBookHit {
    PaletteBookHit {
        id: 7,
        uuid: "library-uuid".into(),
        title: "The Robin on the Oak Throne".into(),
        author_display: "Rebecca Yarros, Someone Else".into(),
        year: Some("2026".into()),
        formats: vec!["EPUB".into()],
        cover_url: Some("/api/covers/library-uuid".into()),
        accent: None,
    }
}

#[test]
fn scan_book_from_hit_splits_the_pre_joined_author_display() {
    let book = scan_book_from_hit(&palette_hit());
    assert_eq!(book.uuid, "library-uuid");
    assert_eq!(book.title, "The Robin on the Oak Throne");
    assert_eq!(
        book.authors,
        vec!["Rebecca Yarros".to_string(), "Someone Else".to_string()]
    );
    assert_eq!(book.cover_url.as_deref(), Some("/api/covers/library-uuid"));
}

#[test]
fn scan_book_from_hit_yields_no_authors_for_an_empty_display() {
    let mut hit = palette_hit();
    hit.author_display = String::new();
    assert!(scan_book_from_hit(&hit).authors.is_empty());
}

#[test]
fn truncation_note_only_speaks_up_when_matches_are_hidden() {
    assert_eq!(truncation_note(5, 5), None);
    assert_eq!(truncation_note(2, 2), None);
    assert!(truncation_note(5, 12)
        .expect("a capped list must say so")
        .contains("Showing 5 of 12"));
}

#[test]
fn go_to_link_opens_the_picker_with_the_scanned_isbn_and_a_way_back() {
    #[component]
    fn AssertGoToLink() -> Element {
        let origin = Stage::Unresolved {
            isbn: "9780441013593".to_string(),
        };
        let state = FlowState {
            stage: Signal::new(origin.clone()),
            isbn: Signal::new("9780441013593".to_string()),
            note: Signal::new(String::new()),
            busy: Signal::new(false),
            error: Signal::new(Some("Could not look that up".to_string())),
        };
        go_to_link(state, "9780441013593".to_string(), origin).call(());

        let Stage::LinkExisting { isbn, origin } = (state.stage)() else {
            panic!("expected the link-existing stage");
        };
        // The copy is filed under the ISBN that was scanned, not under
        // whatever identifier the picked library book happens to publish.
        assert_eq!(isbn, "9780441013593");
        assert!(matches!(*origin, Stage::Unresolved { .. }));
        assert_eq!((state.error)(), None);

        rsx! {}
    }
    VirtualDom::new(AssertGoToLink).rebuild_in_place();
}

#[test]
fn byline_joins_names_with_a_trailing_and() {
    assert_eq!(byline(&[]), "");
    assert_eq!(byline(&["A".into()]), "A");
    assert_eq!(byline(&["A".into(), "B".into()]), "A and B");
    assert_eq!(byline(&["A".into(), "B".into(), "C".into()]), "A, B and C");
    assert_eq!(byline(&["A".into(), " ".into()]), "A");
}
