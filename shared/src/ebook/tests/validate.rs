//! `MetadataOverrides::validate()` tests: length caps (chars not bytes),
//! ISBN-13/ISBN-10 shape, subject/creator/tag limits, print-pages range,
//! and genre count/length limits.

use super::super::*;
use super::contributor;

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

// --- validate() genre limits --------------------------------------------

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
