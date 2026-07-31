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
