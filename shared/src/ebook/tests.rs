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
        isbn10: Some("0134685997".into()),
        creators: Some(vec![contributor("Ada Lovelace")]),
        subjects: Some(vec!["fiction".into(), "history".into()]),
        genres: Some(vec!["Historical Fiction".into()]),
        print_pages: Some(350),
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

#[test]
fn validate_accepts_a_well_formed_isbn10_with_digit_check_digit() {
    let ov = MetadataOverrides {
        isbn10: Some("0134685997".to_string()),
        ..Default::default()
    };
    assert_eq!(ov.validate(), Ok(()));
}

#[test]
fn validate_accepts_an_isbn10_with_x_check_digit() {
    let ov = MetadataOverrides {
        isbn10: Some("020163361X".to_string()),
        ..Default::default()
    };
    assert_eq!(ov.validate(), Ok(()));
}

#[test]
fn validate_accepts_empty_isbn10_as_a_clear() {
    // AC2: an empty string is the "clear the override" sentinel, mirroring
    // isbn13, not an invalid value.
    let ov = MetadataOverrides {
        isbn10: Some(String::new()),
        ..Default::default()
    };
    assert_eq!(ov.validate(), Ok(()));
}

#[test]
fn validate_accepts_missing_isbn10_field() {
    assert_eq!(MetadataOverrides::default().validate(), Ok(()));
}

#[test]
fn validate_rejects_isbn10_with_nine_characters() {
    let ov = MetadataOverrides {
        isbn10: Some("013468599".to_string()),
        ..Default::default()
    };
    let err = ov
        .validate()
        .expect_err("9-char ISBN-10 should be rejected");
    assert!(err.contains("ISBN-10"), "unexpected message: {err}");
}

#[test]
fn validate_rejects_isbn10_with_a_check_digit_x_in_a_non_final_position() {
    let ov = MetadataOverrides {
        isbn10: Some("X134685997".to_string()),
        ..Default::default()
    };
    assert!(
        ov.validate().is_err(),
        "X is only a legal character in the final position"
    );
}

#[test]
fn validate_rejects_isbn10_containing_non_digit_characters() {
    let ov = MetadataOverrides {
        isbn10: Some("01346-8599".to_string()),
        ..Default::default()
    };
    assert!(
        ov.validate().is_err(),
        "a hyphen is not a valid ISBN-10 character"
    );
}

#[test]
fn validate_accepts_print_pages_within_range() {
    let ov = MetadataOverrides {
        print_pages: Some(320),
        ..Default::default()
    };
    assert_eq!(ov.validate(), Ok(()));
}

#[test]
fn validate_accepts_missing_print_pages_field() {
    assert_eq!(MetadataOverrides::default().validate(), Ok(()));
}

#[test]
fn validate_rejects_print_pages_above_the_max() {
    let ov = MetadataOverrides {
        print_pages: Some(MetadataOverrides::PRINT_PAGES_MAX + 1),
        ..Default::default()
    };
    let err = ov
        .validate()
        .expect_err("over-max page count should be rejected");
    assert!(
        err.contains("print page count"),
        "unexpected message: {err}"
    );
}

#[test]
fn validate_accepts_print_pages_at_the_max() {
    let ov = MetadataOverrides {
        print_pages: Some(MetadataOverrides::PRINT_PAGES_MAX),
        ..Default::default()
    };
    assert_eq!(ov.validate(), Ok(()));
}

#[test]
fn validate_rejects_print_pages_at_zero() {
    let ov = MetadataOverrides {
        print_pages: Some(0),
        ..Default::default()
    };
    let err = ov
        .validate()
        .expect_err("zero page count should be rejected");
    assert!(
        err.contains("print page count"),
        "unexpected message: {err}"
    );
}

#[test]
fn validate_rejects_negative_print_pages() {
    let ov = MetadataOverrides {
        print_pages: Some(-1),
        ..Default::default()
    };
    assert!(
        ov.validate().is_err(),
        "a negative page count should be rejected"
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

#[test]
fn merge_incoming_isbn10_wins_over_base_isbn10() {
    let base = MetadataOverrides {
        isbn10: Some("0134685997".into()),
        ..Default::default()
    };
    let incoming = MetadataOverrides {
        isbn10: Some("020163361X".into()),
        ..Default::default()
    };
    let merged = base.merge(&incoming);
    assert_eq!(merged.isbn10, Some("020163361X".into()));
}

#[test]
fn merge_none_isbn10_preserves_base_isbn10() {
    let base = MetadataOverrides {
        isbn10: Some("0134685997".into()),
        ..Default::default()
    };
    let incoming = MetadataOverrides::default();
    let merged = base.merge(&incoming);
    assert_eq!(merged.isbn10, Some("0134685997".into()));
}

#[test]
fn merge_incoming_print_pages_wins_over_base_print_pages() {
    let base = MetadataOverrides {
        print_pages: Some(320),
        ..Default::default()
    };
    let incoming = MetadataOverrides {
        print_pages: Some(512),
        ..Default::default()
    };
    let merged = base.merge(&incoming);
    assert_eq!(merged.print_pages, Some(512));
}

#[test]
fn merge_none_print_pages_preserves_base_print_pages() {
    let base = MetadataOverrides {
        print_pages: Some(320),
        ..Default::default()
    };
    let incoming = MetadataOverrides::default();
    let merged = base.merge(&incoming);
    assert_eq!(merged.print_pages, Some(320));
}

#[test]
fn merge_a_payload_that_omits_all_three_new_fields_preserves_them() {
    // AC2: a client that predates these fields (notably iOS) must not
    // clobber them by omission — the whole point `merge` exists for.
    let base = MetadataOverrides {
        genres: Some(vec!["Historical Fiction".into()]),
        isbn10: Some("0134685997".into()),
        print_pages: Some(320),
        ..Default::default()
    };
    let incoming = MetadataOverrides {
        title: Some("New Title".into()),
        ..Default::default()
    };
    let merged = base.merge(&incoming);
    assert_eq!(merged.title, Some("New Title".into()));
    assert_eq!(merged.genres, Some(vec!["Historical Fiction".into()]));
    assert_eq!(merged.isbn10, Some("0134685997".into()));
    assert_eq!(merged.print_pages, Some(320));
}

// --- BulkMetadataEdit ---

fn tags(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

#[test]
fn bulk_metadata_edit_is_empty_only_when_nothing_is_set() {
    assert!(BulkMetadataEdit::default().is_empty());
    let cases = [
        BulkMetadataEdit {
            authors: Some(vec!["A".into()]),
            ..Default::default()
        },
        BulkMetadataEdit {
            series: Some("S".into()),
            ..Default::default()
        },
        BulkMetadataEdit {
            publisher: Some("P".into()),
            ..Default::default()
        },
        BulkMetadataEdit {
            language: Some("en".into()),
            ..Default::default()
        },
        BulkMetadataEdit {
            add_tags: tags(&["t"]),
            ..Default::default()
        },
        BulkMetadataEdit {
            remove_tags: tags(&["t"]),
            ..Default::default()
        },
    ];
    for edit in cases {
        assert!(!edit.is_empty(), "expected non-empty: {edit:?}");
    }
}

#[test]
fn bulk_metadata_edit_validate_accepts_well_formed_edit() {
    let edit = BulkMetadataEdit {
        authors: Some(vec!["Ursula K. Le Guin".into()]),
        series: Some("Earthsea".into()),
        publisher: Some("Harcourt".into()),
        language: Some("en".into()),
        add_tags: tags(&["fantasy"]),
        remove_tags: tags(&["scifi"]),
        add_genres: tags(&["Fantasy"]),
        remove_genres: tags(&["Sci-Fi"]),
    };
    assert!(edit.validate().is_ok());
}

#[test]
fn bulk_metadata_edit_validate_rejects_overlong_scalar_and_tag_fields() {
    let long_name = "x".repeat(MetadataOverrides::NAME_MAX_LEN + 1);
    let long_tag = "x".repeat(MetadataOverrides::TAG_MAX_LEN + 1);
    let overlong = [
        BulkMetadataEdit {
            series: Some(long_name.clone()),
            ..Default::default()
        },
        BulkMetadataEdit {
            publisher: Some(long_name.clone()),
            ..Default::default()
        },
        BulkMetadataEdit {
            language: Some(long_name.clone()),
            ..Default::default()
        },
        BulkMetadataEdit {
            authors: Some(vec![long_name.clone()]),
            ..Default::default()
        },
        BulkMetadataEdit {
            add_tags: vec![long_tag.clone()],
            ..Default::default()
        },
        BulkMetadataEdit {
            remove_tags: vec![long_tag],
            ..Default::default()
        },
    ];
    for edit in overlong {
        assert!(edit.validate().is_err(), "expected rejection: {edit:?}");
    }
}

#[test]
fn bulk_metadata_edit_scalar_overrides_maps_authors_to_creators_and_leaves_subjects_none() {
    let edit = BulkMetadataEdit {
        authors: Some(vec!["Ada Lovelace".into(), "Grace Hopper".into()]),
        publisher: Some("Bulk House".into()),
        add_tags: tags(&["ignored-here"]),
        ..Default::default()
    };
    let overrides = edit.scalar_overrides();
    let creators = overrides.creators.expect("creators set");
    assert_eq!(creators.len(), 2);
    assert_eq!(creators[0].name, "Ada Lovelace");
    assert_eq!(creators[0].role, None);
    assert_eq!(overrides.publisher, Some("Bulk House".into()));
    assert_eq!(overrides.subjects, None);
    assert_eq!(overrides.title, None);
}

#[test]
fn bulk_metadata_edit_apply_tags_adds_missing_removes_matching_and_never_duplicates() {
    let edit = BulkMetadataEdit {
        add_tags: tags(&["fantasy", "classic"]),
        remove_tags: tags(&["scifi"]),
        ..Default::default()
    };
    let result = edit.apply_tags(&tags(&["scifi", "classic", "adventure"]));
    assert_eq!(result, tags(&["classic", "adventure", "fantasy"]));
}

#[test]
fn bulk_metadata_edit_apply_tags_add_and_remove_of_same_tag_readds_it() {
    let edit = BulkMetadataEdit {
        add_tags: tags(&["fantasy"]),
        remove_tags: tags(&["fantasy"]),
        ..Default::default()
    };
    assert_eq!(edit.apply_tags(&tags(&["fantasy"])), tags(&["fantasy"]));
    assert_eq!(edit.apply_tags(&[]), tags(&["fantasy"]));
}

#[test]
fn bulk_metadata_edit_validate_caps_each_tag_list_separately_not_combined() {
    // A large remove plus a large add is legal even though the two lists
    // together exceed MAX_SUBJECTS — they are deltas, not a subject list;
    // the per-book cap is enforced by the db merge on the true result.
    let forty: Vec<String> = (0..40).map(|i| format!("tag{i}")).collect();
    let edit = BulkMetadataEdit {
        add_tags: forty.clone(),
        remove_tags: forty,
        ..Default::default()
    };
    assert!(edit.validate().is_ok());

    let oversized: Vec<String> = (0..MetadataOverrides::MAX_SUBJECTS + 1)
        .map(|i| format!("tag{i}"))
        .collect();
    let edit = BulkMetadataEdit {
        add_tags: oversized,
        ..Default::default()
    };
    assert!(edit.validate().is_err());
}

#[test]
fn validate_rejects_too_many_genres() {
    let ov = MetadataOverrides {
        genres: Some(vec!["genre".to_string(); MetadataOverrides::MAX_GENRES + 1]),
        ..Default::default()
    };
    let err = ov.validate().expect_err("over-cap genre list rejected");
    assert!(err.contains("too many genres"), "unexpected message: {err}");
}

#[test]
fn validate_rejects_overlong_genre_and_accepts_one_at_the_cap() {
    let ov = MetadataOverrides {
        genres: Some(vec!["x".repeat(MetadataOverrides::GENRE_MAX_LEN + 1)]),
        ..Default::default()
    };
    let err = ov.validate().expect_err("over-length genre rejected");
    assert!(err.contains("genre exceeds"), "unexpected message: {err}");

    let ov = MetadataOverrides {
        genres: Some(vec!["x".repeat(MetadataOverrides::GENRE_MAX_LEN)]),
        ..Default::default()
    };
    assert!(ov.validate().is_ok());
}

#[test]
fn merge_incoming_genres_replace_base_genres_wholesale() {
    let base = MetadataOverrides {
        genres: Some(tags(&["Horror", "Mystery"])),
        ..Default::default()
    };
    let incoming = MetadataOverrides {
        genres: Some(tags(&["Sci-Fi"])),
        ..Default::default()
    };
    assert_eq!(base.merge(&incoming).genres, Some(tags(&["Sci-Fi"])));
}

#[test]
fn merge_preserves_base_genres_when_incoming_leaves_them_untouched() {
    let base = MetadataOverrides {
        genres: Some(tags(&["Horror"])),
        ..Default::default()
    };
    let incoming = MetadataOverrides {
        title: Some("New Title".into()),
        ..Default::default()
    };
    let merged = base.merge(&incoming);
    assert_eq!(merged.genres, Some(tags(&["Horror"])));
    assert_eq!(merged.title, Some("New Title".into()));
}

#[test]
fn bulk_metadata_edit_apply_genres_adds_missing_removes_matching_and_never_duplicates() {
    let edit = BulkMetadataEdit {
        add_genres: tags(&["Fantasy", "Mystery"]),
        remove_genres: tags(&["Horror"]),
        ..Default::default()
    };
    let result = edit.apply_genres(&tags(&["Horror", "Mystery", "Romance"]));
    assert_eq!(result, tags(&["Mystery", "Romance", "Fantasy"]));
}

#[test]
fn bulk_metadata_edit_is_empty_is_false_when_only_genre_deltas_are_set() {
    let edit = BulkMetadataEdit {
        add_genres: tags(&["Fantasy"]),
        ..Default::default()
    };
    assert!(!edit.is_empty());
    assert!(BulkMetadataEdit::default().is_empty());
}

#[test]
fn bulk_metadata_edit_validate_caps_each_genre_list_separately_not_combined() {
    let forty: Vec<String> = (0..40).map(|i| format!("genre{i}")).collect();
    let edit = BulkMetadataEdit {
        add_genres: forty.clone(),
        remove_genres: forty,
        ..Default::default()
    };
    assert!(edit.validate().is_ok());

    let oversized: Vec<String> = (0..MetadataOverrides::MAX_GENRES + 1)
        .map(|i| format!("genre{i}"))
        .collect();
    let edit = BulkMetadataEdit {
        add_genres: oversized,
        ..Default::default()
    };
    assert!(edit.validate().is_err());
}
