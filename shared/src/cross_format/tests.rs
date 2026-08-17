//! Unit tests for the cross-format wire-type validators: the per-format
//! position boundaries `DeclareSyncPoint::validate` enforces, and the
//! primary-narration and duplicate-ordinal rules in
//! `ConfirmCrossFormatLink::validate`.

use super::*;

fn reading_declaration(ebook_fraction: Option<f64>) -> DeclareSyncPoint {
    DeclareSyncPoint {
        book_uuid: "uuid-1".into(),
        format: ProgressFormat::Epub,
        ebook_fraction,
        epub_cfi: None,
        audio_book_file_id: None,
        audio_seconds: None,
    }
}

fn listening_declaration(audio_seconds: Option<f64>) -> DeclareSyncPoint {
    DeclareSyncPoint {
        book_uuid: "uuid-1".into(),
        format: ProgressFormat::Audio,
        ebook_fraction: None,
        epub_cfi: None,
        audio_book_file_id: Some(7),
        audio_seconds,
    }
}

fn ok_link(mode: CrossFormatLinkMode) -> ConfirmCrossFormatLink {
    ConfirmCrossFormatLink {
        book_uuid: "uuid-1".into(),
        mode,
        primary_book_file_id: None,
        audio_order: None,
    }
}

#[test]
fn declare_sync_point_validate_accepts_ebook_fraction_inside_range() {
    assert!(reading_declaration(Some(0.5)).validate().is_ok());
}

#[test]
fn declare_sync_point_validate_accepts_ebook_fraction_at_both_bounds() {
    assert!(reading_declaration(Some(0.0)).validate().is_ok());
    assert!(reading_declaration(Some(1.0)).validate().is_ok());
}

#[test]
fn declare_sync_point_validate_rejects_ebook_fraction_above_one() {
    let err = reading_declaration(Some(1.1))
        .validate()
        .expect_err("a fraction past the end of the book must be rejected");
    assert!(err.contains("ebook_fraction"), "got: {err}");
}

#[test]
fn declare_sync_point_validate_rejects_ebook_fraction_below_zero() {
    let err = reading_declaration(Some(-0.1))
        .validate()
        .expect_err("a negative fraction must be rejected");
    assert!(err.contains("ebook_fraction"), "got: {err}");
}

#[test]
fn declare_sync_point_validate_rejects_nan_ebook_fraction() {
    let err = reading_declaration(Some(f64::NAN))
        .validate()
        .expect_err("NaN compares false against the range and must be rejected");
    assert!(err.contains("ebook_fraction"), "got: {err}");
}

#[test]
fn declare_sync_point_validate_rejects_reading_declaration_without_a_fraction() {
    let err = reading_declaration(None)
        .validate()
        .expect_err("a reading declaration naming no position must be rejected");
    assert!(err.contains("ebook_fraction"), "got: {err}");
}

#[test]
fn declare_sync_point_validate_accepts_positive_audio_seconds() {
    assert!(listening_declaration(Some(1234.5)).validate().is_ok());
}

#[test]
fn declare_sync_point_validate_accepts_audio_seconds_at_zero() {
    assert!(listening_declaration(Some(0.0)).validate().is_ok());
}

#[test]
fn declare_sync_point_validate_rejects_negative_audio_seconds() {
    let err = listening_declaration(Some(-1.0))
        .validate()
        .expect_err("a position before the start of the file must be rejected");
    assert!(err.contains("audio_seconds"), "got: {err}");
}

#[test]
fn declare_sync_point_validate_rejects_nan_audio_seconds() {
    let err = listening_declaration(Some(f64::NAN))
        .validate()
        .expect_err("NaN compares false against the floor and must be rejected");
    assert!(err.contains("audio_seconds"), "got: {err}");
}

#[test]
fn declare_sync_point_validate_rejects_listening_declaration_without_seconds() {
    let err = listening_declaration(None)
        .validate()
        .expect_err("a listening declaration naming no position must be rejected");
    assert!(err.contains("audio_seconds"), "got: {err}");
}

#[test]
fn declare_sync_point_validate_checks_only_the_declaring_surfaces_half() {
    // The counterpart half comes from the server's stored row, so an
    // out-of-range value in the other format's field is not this
    // declaration's business.
    let mut d = listening_declaration(Some(10.0));
    d.ebook_fraction = Some(99.0);
    assert!(d.validate().is_ok());

    let mut d = reading_declaration(Some(0.25));
    d.audio_seconds = Some(-5.0);
    assert!(d.validate().is_ok());
}

#[test]
fn confirm_cross_format_link_validate_accepts_sequence_mode_without_a_primary() {
    assert!(ok_link(CrossFormatLinkMode::Sequence).validate().is_ok());
}

#[test]
fn confirm_cross_format_link_validate_rejects_narrations_mode_without_a_primary() {
    let err = ok_link(CrossFormatLinkMode::Narrations)
        .validate()
        .expect_err("narrations mode with no primary must be rejected");
    assert!(err.contains("primary narration"), "got: {err}");
}

#[test]
fn confirm_cross_format_link_validate_accepts_narrations_mode_with_a_primary() {
    let mut c = ok_link(CrossFormatLinkMode::Narrations);
    c.primary_book_file_id = Some(3);
    assert!(c.validate().is_ok());
}

#[test]
fn confirm_cross_format_link_validate_accepts_an_audio_order_of_distinct_ids() {
    let mut c = ok_link(CrossFormatLinkMode::Sequence);
    c.audio_order = Some(vec![3, 1, 2]);
    assert!(c.validate().is_ok());
}

#[test]
fn confirm_cross_format_link_validate_rejects_an_audio_order_repeating_an_id() {
    let mut c = ok_link(CrossFormatLinkMode::Sequence);
    c.audio_order = Some(vec![1, 2, 1]);
    let err = c
        .validate()
        .expect_err("an order list that repeats a file must be rejected");
    assert!(err.contains("repeat"), "got: {err}");
}
