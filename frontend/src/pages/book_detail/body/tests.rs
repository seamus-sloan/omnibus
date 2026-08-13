//! Tests for `bd_identifier_key`: the rendered-list key for a book's
//! identifiers must stay unique across same-scheme, schemeless, and
//! delimiter-containing values so React/Dioxus keying never collides.

use super::*;

#[test]
fn bd_identifier_key_is_unique_for_same_scheme_distinct_values() {
    // The book-detail crash repro: two `unknown`-scheme identifiers on
    // one book must not collide on the rendered list key.
    let isbn = Identifier {
        value: "978-1-938570-40-7".into(),
        scheme: Some("unknown".into()),
    };
    let urn = Identifier {
        value: "urn:uuid:c0e51a66-085f-4805-b116-a0d451d281bd".into(),
        scheme: Some("unknown".into()),
    };
    assert_ne!(bd_identifier_key(&isbn), bd_identifier_key(&urn));
}

#[test]
fn bd_identifier_key_is_unique_for_schemeless_distinct_values() {
    let a = Identifier {
        value: "a".into(),
        scheme: None,
    };
    let b = Identifier {
        value: "b".into(),
        scheme: None,
    };
    assert_ne!(bd_identifier_key(&a), bd_identifier_key(&b));
}

#[test]
fn bd_identifier_key_does_not_collide_when_data_contains_the_delimiter() {
    // A naive `scheme|value` join would map both of these to "a|b|c";
    // the `Debug`-quoted encoding keeps them distinct.
    let split_scheme = Identifier {
        value: "c".into(),
        scheme: Some("a|b".into()),
    };
    let split_value = Identifier {
        value: "b|c".into(),
        scheme: Some("a".into()),
    };
    assert_ne!(
        bd_identifier_key(&split_scheme),
        bd_identifier_key(&split_value)
    );
}

#[test]
fn bd_identifier_label_maps_unknown_scheme_isbn10_shape_to_isbn() {
    // #1910 repro: the sync layer's "unknown" scheme placeholder on an
    // ISBN-10-shaped value must never surface as the literal word "unknown".
    let ident = Identifier {
        value: "0-306-40615-2".into(),
        scheme: Some("unknown".into()),
    };
    assert_eq!(bd_identifier_label(&ident), "ISBN");
}

#[test]
fn bd_identifier_label_maps_unknown_scheme_isbn13_shape_to_isbn() {
    // The Feywild Job repro from the issue: a hyphenated ISBN-13 under the
    // "unknown" scheme.
    let ident = Identifier {
        value: "978-0-593-59980-8".into(),
        scheme: Some("unknown".into()),
    };
    assert_eq!(bd_identifier_label(&ident), "ISBN");
}

#[test]
fn bd_identifier_label_maps_unknown_scheme_isbn10_checkdigit_x_to_isbn() {
    let ident = Identifier {
        value: "080442957X".into(),
        scheme: Some("unknown".into()),
    };
    assert_eq!(bd_identifier_label(&ident), "ISBN");
}

#[test]
fn bd_identifier_label_rejects_13char_value_with_trailing_x() {
    // A trailing check-digit X is only valid for ISBN-10; a 13-char value
    // ending in X is not a real ISBN-13 and must fall through to generic.
    let ident = Identifier {
        value: "978030640615X".into(),
        scheme: Some("unknown".into()),
    };
    assert_eq!(bd_identifier_label(&ident), "Identifier");
}

#[test]
fn bd_identifier_label_maps_non_isbn_unknown_scheme_to_generic_identifier() {
    let ident = Identifier {
        value: "urn:uuid:c0e51a66-085f-4805-b116-a0d451d281bd".into(),
        scheme: Some("unknown".into()),
    };
    assert_eq!(bd_identifier_label(&ident), "Identifier");
}

#[test]
fn bd_identifier_label_maps_missing_scheme_by_value_shape() {
    let isbn = Identifier {
        value: "9780593599808".into(),
        scheme: None,
    };
    let other = Identifier {
        value: "some-other-id".into(),
        scheme: None,
    };
    assert_eq!(bd_identifier_label(&isbn), "ISBN");
    assert_eq!(bd_identifier_label(&other), "Identifier");
}

#[test]
fn bd_identifier_label_keeps_known_scheme_as_is() {
    let ident = Identifier {
        value: "https://example.com/book".into(),
        scheme: Some("URL".into()),
    };
    assert_eq!(bd_identifier_label(&ident), "URL");
}
