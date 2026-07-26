//! Serde/token round-trip tests plus `UpdateCopyNoteRequest::validate` length-cap
//! coverage.

use super::*;

#[test]
fn wishlist_source_round_trips_through_the_db_string_for_every_variant() {
    for variant in [
        WishlistSource::Scan,
        WishlistSource::Detail,
        WishlistSource::Manual,
    ] {
        assert_eq!(
            WishlistSource::from_db(variant.as_str()),
            Some(variant),
            "variant {variant:?} did not round-trip through as_str/from_db"
        );
    }
}

#[test]
fn wishlist_source_from_db_returns_none_for_unrecognized_token() {
    assert_eq!(WishlistSource::from_db("bogus"), None);
}

#[test]
fn update_copy_note_request_validate_accepts_a_missing_note() {
    let req = UpdateCopyNoteRequest { note: None };
    assert!(req.validate().is_ok());
}

#[test]
fn update_copy_note_request_validate_accepts_a_well_formed_note() {
    let req = UpdateCopyNoteRequest {
        note: Some("1st edition, dust jacket a little worn.".into()),
    };
    assert!(req.validate().is_ok());
}

#[test]
fn update_copy_note_request_validate_rejects_an_oversized_note() {
    let req = UpdateCopyNoteRequest {
        note: Some("x".repeat(UpdateCopyNoteRequest::NOTE_MAX_LEN + 1)),
    };
    let err = req.validate().expect_err("oversized note must be rejected");
    assert!(err.contains("note"), "got: {err}");
}
