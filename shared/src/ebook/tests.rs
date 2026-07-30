//! Tests for `EbookMetadata` and `MetadataOverrides`: title/description
//! display fallbacks, override validation (length caps measured in chars,
//! ISBN-13 shape, subject/creator/tag limits), and override-merge layering.

use super::*;

fn contributor(name: &str) -> Contributor {
    Contributor {
        name: name.to_string(),
        ..Default::default()
    }
}

// --- display_title() (free fn — label/filename pairs outside EbookMetadata, e.g. book_files) ---

#[test]
fn display_title_helper_returns_title_when_set() {
    assert_eq!(display_title(Some("A Title"), "file.epub"), "A Title");
}

#[test]
fn display_title_helper_falls_back_to_filename_when_title_is_none() {
    assert_eq!(display_title(None, "file.epub"), "file.epub");
}

// --- EbookMetadata::display_title() ------------------------------------

#[test]
fn display_title_returns_title_when_present() {
    let m = EbookMetadata {
        filename: "book.epub".into(),
        title: Some("The Actual Title".into()),
        ..Default::default()
    };
    assert_eq!(m.display_title(), "The Actual Title");
}

#[test]
fn display_title_falls_back_to_filename_when_title_is_none() {
    let m = EbookMetadata {
        filename: "untitled.epub".into(),
        title: None,
        ..Default::default()
    };
    assert_eq!(m.display_title(), "untitled.epub");
}

// --- validate() --------------------------------------------------------

#[test]
fn validate_accepts_well_formed_override() {
    let ov = MetadataOverrides {
        title: Some("A Reasonable Title".into()),
        description: Some("A short blurb.".into()),
        publisher: Some("Some Press".into()),
        published: Some("2020".into()),
        language: Some("en".into()),
        series: Some("Some Series".into()),
        series_index: Some("1".into()),
        isbn13: Some("9780134685991".into()),
        creators: Some(vec![contributor("Ada Lovelace")]),
        subjects: Some(vec!["fiction".into(), "history".into()]),
    };
    assert_eq!(ov.validate(), Ok(()));
}

#[test]
fn validate_accepts_empty_default_override() {
    // An all-`None` override (nothing being overridden) is trivially valid.
    assert_eq!(MetadataOverrides::default().validate(), Ok(()));
}

#[test]
fn validate_rejects_title_exceeding_length_cap() {
    let ov = MetadataOverrides {
        title: Some("x".repeat(MetadataOverrides::TITLE_MAX_LEN + 1)),
        ..Default::default()
    };
    let err = ov
        .validate()
        .expect_err("over-length title should be rejected");
    assert!(
        err.contains("title"),
        "message should name the field: {err}"
    );
    assert!(err.contains("500"), "message should name the cap: {err}");
}

#[test]
fn validate_accepts_title_at_length_cap() {
    // Boundary: exactly TITLE_MAX_LEN is allowed (the check is `> max`).
    let ov = MetadataOverrides {
        title: Some("x".repeat(MetadataOverrides::TITLE_MAX_LEN)),
        ..Default::default()
    };
    assert_eq!(ov.validate(), Ok(()));
}

#[test]
fn validate_rejects_description_exceeding_length_cap() {
    let ov = MetadataOverrides {
        description: Some("x".repeat(MetadataOverrides::DESCRIPTION_MAX_LEN + 1)),
        ..Default::default()
    };
    let err = ov
        .validate()
        .expect_err("over-length description should be rejected");
    assert!(
        err.contains("description"),
        "message should name the field: {err}"
    );
    assert!(err.contains("50000"), "message should name the cap: {err}");
}

#[test]
fn validate_accepts_description_at_length_cap() {
    // Boundary: exactly DESCRIPTION_MAX_LEN is allowed.
    let ov = MetadataOverrides {
        description: Some("x".repeat(MetadataOverrides::DESCRIPTION_MAX_LEN)),
        ..Default::default()
    };
    assert_eq!(ov.validate(), Ok(()));
}

#[test]
fn validate_rejects_series_name_exceeding_length_cap() {
    let ov = MetadataOverrides {
        series: Some("x".repeat(MetadataOverrides::NAME_MAX_LEN + 1)),
        ..Default::default()
    };
    let err = ov
        .validate()
        .expect_err("over-length series should be rejected");
    assert!(
        err.contains("series"),
        "message should name the field: {err}"
    );
    assert!(err.contains("250"), "message should name the cap: {err}");
}

#[test]
fn validate_measures_title_cap_in_chars_not_bytes() {
    // A 4-byte emoji is one Unicode scalar. Exactly the cap in chars must
    // pass even though it is 4× the cap in bytes.
    let at_cap = "\u{1F600}".repeat(MetadataOverrides::TITLE_MAX_LEN);
    assert_eq!(at_cap.chars().count(), MetadataOverrides::TITLE_MAX_LEN);
    assert!(at_cap.len() > MetadataOverrides::TITLE_MAX_LEN);
    let ov = MetadataOverrides {
        title: Some(at_cap),
        ..Default::default()
    };
    assert_eq!(
        ov.validate(),
        Ok(()),
        "exactly the char cap should pass regardless of byte count"
    );

    // cap + 1 chars must fail even though the same byte count of ASCII
    // would have been accepted.
    let over_cap = "\u{1F600}".repeat(MetadataOverrides::TITLE_MAX_LEN + 1);
    let ov = MetadataOverrides {
        title: Some(over_cap),
        ..Default::default()
    };
    assert!(
        ov.validate().is_err(),
        "char cap + 1 should be rejected regardless of byte count"
    );
}

#[test]
fn validate_measures_description_cap_in_chars_not_bytes() {
    // CJK characters are 3 bytes each in UTF-8. Exactly the description cap
    // in chars passes; cap + 1 fails — neither decided by byte length.
    let at_cap = "\u{4E16}".repeat(MetadataOverrides::DESCRIPTION_MAX_LEN);
    assert!(at_cap.len() > MetadataOverrides::DESCRIPTION_MAX_LEN);
    let ov = MetadataOverrides {
        description: Some(at_cap),
        ..Default::default()
    };
    assert_eq!(ov.validate(), Ok(()));

    let over_cap = "\u{4E16}".repeat(MetadataOverrides::DESCRIPTION_MAX_LEN + 1);
    let ov = MetadataOverrides {
        description: Some(over_cap),
        ..Default::default()
    };
    assert!(ov.validate().is_err());
}

#[test]
fn validate_rejects_too_many_subjects() {
    let ov = MetadataOverrides {
        subjects: Some(vec!["tag".to_string(); MetadataOverrides::MAX_SUBJECTS + 1]),
        ..Default::default()
    };
    let err = ov
        .validate()
        .expect_err("over-cap subject count should be rejected");
    assert!(err.contains("too many tags"), "unexpected message: {err}");
}

#[test]
fn validate_rejects_over_long_creator_name() {
    let ov = MetadataOverrides {
        creators: Some(vec![contributor(
            &"x".repeat(MetadataOverrides::NAME_MAX_LEN + 1),
        )]),
        ..Default::default()
    };
    let err = ov
        .validate()
        .expect_err("over-length creator name should be rejected");
    assert!(err.contains("creator name"), "unexpected message: {err}");
}

#[test]
fn validate_rejects_over_long_creator_role() {
    let ov = MetadataOverrides {
        creators: Some(vec![Contributor {
            name: "Ada Lovelace".into(),
            role: Some("x".repeat(MetadataOverrides::NAME_MAX_LEN + 1)),
            ..Default::default()
        }]),
        ..Default::default()
    };
    let err = ov
        .validate()
        .expect_err("over-length creator role should be rejected");
    assert!(err.contains("creator role"), "unexpected message: {err}");
}

#[test]
fn validate_rejects_over_long_creator_file_as() {
    let ov = MetadataOverrides {
        creators: Some(vec![Contributor {
            name: "Ada Lovelace".into(),
            file_as: Some("x".repeat(MetadataOverrides::NAME_MAX_LEN + 1)),
            ..Default::default()
        }]),
        ..Default::default()
    };
    let err = ov
        .validate()
        .expect_err("over-length creator file_as should be rejected");
    assert!(err.contains("creator file_as"), "unexpected message: {err}");
}

#[test]
fn validate_rejects_over_long_tag() {
    // TAG_MAX_LEN = 128 (restored from the pre-existing MAX_SUBJECT_CHARS cap).
    // Over-limit boundary: 129 chars must be rejected.
    let ov = MetadataOverrides {
        subjects: Some(vec!["x".repeat(MetadataOverrides::TAG_MAX_LEN + 1)]),
        ..Default::default()
    };
    let err = ov
        .validate()
        .expect_err("over-length tag should be rejected");
    assert!(err.contains("tag"), "message should name the field: {err}");
    assert!(err.contains("128"), "message should name the cap: {err}");
}

#[test]
fn validate_accepts_tag_at_length_cap() {
    // Boundary: exactly TAG_MAX_LEN (128) chars is allowed.
    let ov = MetadataOverrides {
        subjects: Some(vec!["x".repeat(MetadataOverrides::TAG_MAX_LEN)]),
        ..Default::default()
    };
    assert_eq!(ov.validate(), Ok(()));
}

#[test]
fn validate_accepts_a_well_formed_13_digit_isbn() {
    let ov = MetadataOverrides {
        isbn13: Some("9780134685991".to_string()),
        ..Default::default()
    };
    assert_eq!(ov.validate(), Ok(()));
}

#[test]
fn validate_accepts_empty_isbn13_as_a_clear() {
    // AC4: an empty string is the "clear the override" sentinel, not an
    // invalid value.
    let ov = MetadataOverrides {
        isbn13: Some(String::new()),
        ..Default::default()
    };
    assert_eq!(ov.validate(), Ok(()));
}

#[test]
fn validate_accepts_missing_isbn13_field() {
    // `None` means "don't touch the ISBN override" — trivially valid.
    assert_eq!(MetadataOverrides::default().validate(), Ok(()));
}

#[test]
fn validate_rejects_isbn13_with_twelve_digits() {
    let ov = MetadataOverrides {
        isbn13: Some("978013468599".to_string()),
        ..Default::default()
    };
    let err = ov.validate().expect_err("12-digit ISBN should be rejected");
    assert!(err.contains("13 digits"), "unexpected message: {err}");
}

#[test]
fn validate_rejects_isbn13_with_fourteen_digits() {
    let ov = MetadataOverrides {
        isbn13: Some("97801346859912".to_string()),
        ..Default::default()
    };
    let err = ov.validate().expect_err("14-digit ISBN should be rejected");
    assert!(err.contains("13 digits"), "unexpected message: {err}");
}

#[test]
fn validate_rejects_isbn13_containing_non_digit_characters() {
    let ov = MetadataOverrides {
        isbn13: Some("978-0134685991".to_string()),
        ..Default::default()
    };
    let err = ov
        .validate()
        .expect_err("hyphenated ISBN should be rejected");
    assert!(err.contains("13 digits"), "unexpected message: {err}");
}

#[test]
fn validate_rejects_isbn13_with_whitespace() {
    let ov = MetadataOverrides {
        isbn13: Some(" 9780134685991".to_string()),
        ..Default::default()
    };
    assert!(
        ov.validate().is_err(),
        "leading whitespace should push the char count/content out of range"
    );
}

// --- merge() -----------------------------------------------------------
//
// NOTE on contract: the issue describes `merge` as "applies an override
// layer onto a base `EbookMetadata`, with empty-array meaning clear-all".
// The actual `MetadataOverrides::merge` merges two `MetadataOverrides`
// layers (a prior stored override + an incoming edit), not an
// `EbookMetadata`, and uses `incoming.field.or(self.field)`. The
// empty-array-clears-the-base-book behaviour lives in
// `db::metadata_overrides::apply_overrides`, where `Some(vec![])`
// overwrites the book's list with an empty vec. The tests below assert the
// ACTUAL `merge` semantics; case (4) documents that for `merge` itself an
// empty `Some(vec![])` is an *override layer value* that wins over the
// base layer (it does not collapse to "don't touch"), which is consistent
// with — and feeds into — the clear-all behaviour at apply time.

#[test]
fn merge_empty_creators_layer_wins_over_base_creators() {
    // Issue case (4): incoming `creators: Some(vec![])` is preserved on the
    // merged override (it does NOT fall back to the base layer's creators).
    // Downstream `apply_overrides` then clears the book's creators.
    let base = MetadataOverrides {
        creators: Some(vec![contributor("Stale Author")]),
        ..Default::default()
    };
    let incoming = MetadataOverrides {
        creators: Some(vec![]),
        ..Default::default()
    };
    let merged = base.merge(&incoming);
    assert_eq!(merged.creators, Some(vec![]));
}

#[test]
fn merge_none_creators_leaves_base_creators_unchanged() {
    // Issue case (5): incoming `creators: None` preserves the base layer.
    let base = MetadataOverrides {
        creators: Some(vec![contributor("Original Author")]),
        ..Default::default()
    };
    let incoming = MetadataOverrides::default();
    let merged = base.merge(&incoming);
    assert_eq!(merged.creators, Some(vec![contributor("Original Author")]));
}

#[test]
fn merge_nonempty_creators_replaces_base_entirely() {
    // Issue case (6): incoming non-empty creators replaces, not appends.
    let base = MetadataOverrides {
        creators: Some(vec![contributor("Old A"), contributor("Old B")]),
        ..Default::default()
    };
    let incoming = MetadataOverrides {
        creators: Some(vec![contributor("New One")]),
        ..Default::default()
    };
    let merged = base.merge(&incoming);
    assert_eq!(merged.creators, Some(vec![contributor("New One")]));
}

#[test]
fn merge_empty_subjects_layer_wins_over_base_subjects() {
    // Adjacent case: same empty-vec-wins semantics for the subjects field.
    let base = MetadataOverrides {
        subjects: Some(vec!["stale".into()]),
        ..Default::default()
    };
    let incoming = MetadataOverrides {
        subjects: Some(vec![]),
        ..Default::default()
    };
    let merged = base.merge(&incoming);
    assert_eq!(merged.subjects, Some(vec![]));
}

#[test]
fn merge_preserves_untouched_scalar_fields_from_base() {
    // An incoming edit that only sets `title` must preserve all other
    // prior-override fields (the documented reason `merge` exists).
    let base = MetadataOverrides {
        title: Some("Old Title".into()),
        publisher: Some("Kept Publisher".into()),
        series: Some("Kept Series".into()),
        ..Default::default()
    };
    let incoming = MetadataOverrides {
        title: Some("New Title".into()),
        ..Default::default()
    };
    let merged = base.merge(&incoming);
    assert_eq!(merged.title, Some("New Title".into()));
    assert_eq!(merged.publisher, Some("Kept Publisher".into()));
    assert_eq!(merged.series, Some("Kept Series".into()));
}

#[test]
fn merge_incoming_isbn13_wins_over_base_isbn13() {
    let base = MetadataOverrides {
        isbn13: Some("9780134685991".into()),
        ..Default::default()
    };
    let incoming = MetadataOverrides {
        isbn13: Some("9780316769488".into()),
        ..Default::default()
    };
    let merged = base.merge(&incoming);
    assert_eq!(merged.isbn13, Some("9780316769488".into()));
}

#[test]
fn merge_none_isbn13_preserves_base_isbn13() {
    let base = MetadataOverrides {
        isbn13: Some("9780134685991".into()),
        ..Default::default()
    };
    let incoming = MetadataOverrides::default();
    let merged = base.merge(&incoming);
    assert_eq!(merged.isbn13, Some("9780134685991".into()));
}
