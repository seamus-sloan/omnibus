//! Tests for `bd_identifier_key`: the rendered-list key for a book's
//! identifiers must stay unique across same-scheme, schemeless, and
//! delimiter-containing values so React/Dioxus keying never collides. Also
//! covers `bd_identifier_label`'s scheme inference.

use super::*;

fn ident(scheme: Option<&str>, value: &str) -> Identifier {
    Identifier {
        scheme: scheme.map(str::to_string),
        value: value.to_string(),
    }
}

#[test]
fn bd_identifier_key_distinguishes_same_scheme_different_values() {
    // The book-detail crash repro: two `unknown`-scheme identifiers on one
    // book must not collide on the rendered list key.
    assert_ne!(
        bd_identifier_key(&ident(Some("unknown"), "978-1-938570-40-7")),
        bd_identifier_key(&ident(
            Some("unknown"),
            "urn:uuid:c0e51a66-085f-4805-b116-a0d451d281bd"
        ))
    );
    assert_ne!(
        bd_identifier_key(&ident(Some("ISBN"), "111")),
        bd_identifier_key(&ident(Some("ISBN"), "222"))
    );
}

#[test]
fn bd_identifier_key_distinguishes_schemeless_values_from_each_other_and_from_a_scheme() {
    assert_ne!(
        bd_identifier_key(&ident(None, "a")),
        bd_identifier_key(&ident(None, "b"))
    );
    assert_ne!(
        bd_identifier_key(&ident(None, "111")),
        bd_identifier_key(&ident(Some("ISBN"), "111"))
    );
}

#[test]
fn bd_identifier_key_survives_delimiter_shuffle_between_fields() {
    // A naive `scheme|value` join would map both of these to "a|b|c"; the
    // `Debug`-quoted encoding keeps them distinct.
    let a = bd_identifier_key(&ident(Some("a\u{1f}b"), "c"));
    let b = bd_identifier_key(&ident(Some("a"), "b\u{1f}c"));
    assert_ne!(a, b);
}

#[test]
fn bd_identifier_label_prefers_a_real_scheme() {
    assert_eq!(bd_identifier_label(&ident(Some("ASIN"), "B000")), "ASIN");
}

#[test]
fn bd_identifier_label_infers_isbn_from_shape_when_scheme_unknown() {
    assert_eq!(
        bd_identifier_label(&ident(Some("unknown"), "978-0-7564-0407-9")),
        "ISBN"
    );
    assert_eq!(bd_identifier_label(&ident(None, "012345678X")), "ISBN");
    assert_eq!(
        bd_identifier_label(&ident(None, "not-an-isbn")),
        "Identifier"
    );
}
