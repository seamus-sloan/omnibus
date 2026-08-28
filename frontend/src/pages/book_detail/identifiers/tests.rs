//! Tests for [`super`]: `bd_identifier_key`'s rendered-list key must stay
//! unique across same-scheme, schemeless, and delimiter-containing values so
//! Dioxus keying never collides; the label maps a scheme to something a
//! reader can read; and `bd_identifier_rows` collapses one identifier listed
//! under several schemes into a single row.

use super::*;

fn ident(scheme: Option<&str>, value: &str) -> Identifier {
    Identifier {
        scheme: scheme.map(str::to_string),
        value: value.to_string(),
    }
}

/// The label half of [`bd_identifier_label_ranked`] — the rank is asserted
/// through `bd_identifier_rows`, which is the only thing that reads it.
fn label(ident: &Identifier) -> String {
    bd_identifier_label_ranked(ident).0
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
    assert_eq!(label(&ident(Some("ASIN"), "B000")), "ASIN");
}

#[test]
fn bd_identifier_label_infers_isbn_from_shape_when_scheme_unknown() {
    assert_eq!(label(&ident(Some("unknown"), "978-0-7564-0407-9")), "ISBN");
    assert_eq!(label(&ident(None, "012345678X")), "ISBN");
    assert_eq!(label(&ident(None, "not-an-isbn")), "Identifier");
}

#[test]
fn bd_identifier_label_names_an_onix_codelist_value() {
    // The reported row labelled "15" — the ONIX codelist-5 code an EPUB 3
    // `identifier-type` refinement carries for an ISBN-13.
    assert_eq!(label(&ident(Some("15"), "9780316769488")), "ISBN-13");
    assert_eq!(label(&ident(Some("02"), "0316769487")), "ISBN-10");
    assert_eq!(label(&ident(Some("06"), "10.1000/182")), "DOI");
}

#[test]
fn bd_identifier_label_never_shows_a_bare_numeric_scheme() {
    // A codelist value this table doesn't know is still not a label — fall
    // back to the value's own shape rather than printing the code.
    assert_eq!(label(&ident(Some("99"), "9780316769488")), "ISBN");
    assert_eq!(label(&ident(Some("99"), "xyz")), "Identifier");
}

#[test]
fn bd_identifier_label_names_the_source_uuid_for_what_it_holds() {
    // Calibre writes its own book uuid under the `uuid` scheme; it is not
    // the book's Omnibus uuid, so the row must not claim to be one.
    assert_eq!(
        label(&ident(Some("uuid"), "c0e51a66-085f-4805-b116-a0d451d281bd")),
        "Source UUID"
    );
    assert_eq!(label(&ident(Some("calibre"), "412")), "Calibre ID");
}

#[test]
fn bd_identifier_label_passes_an_unrecognized_named_scheme_through() {
    assert_eq!(label(&ident(Some("BNB"), "GB1234")), "BNB");
}

#[test]
fn bd_identifier_rows_collapse_one_value_listed_under_several_schemes() {
    // An EPUB 3 package writes its ISBN as a `<dc:identifier>` and again as
    // an ONIX refinement; both reached the table as separate rows.
    let rows = bd_identifier_rows(&[
        ident(Some("15"), "9780316769488"),
        ident(Some("ISBN"), "9780316769488"),
    ]);
    assert_eq!(rows.len(), 1);
    // Both schemes are ones the table knows, so the first wins — and either
    // way the surviving label is a name, never the code.
    assert_eq!(rows[0].label, "ISBN-13");
    assert_eq!(rows[0].value, "9780316769488");
}

#[test]
fn bd_identifier_rows_prefer_a_known_label_over_an_inferred_one() {
    let rows = bd_identifier_rows(&[
        ident(Some("unknown"), "9780316769488"),
        ident(Some("15"), "9780316769488"),
    ]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "ISBN-13");
}

#[test]
fn bd_identifier_rows_keep_the_first_occurrence_on_a_tie() {
    let rows = bd_identifier_rows(&[
        ident(Some("ISBN"), "9780316769488"),
        ident(Some("isbn"), "9780316769488"),
    ]);
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].label, "ISBN");
}

#[test]
fn bd_identifier_rows_keep_distinct_values_in_source_order() {
    let rows = bd_identifier_rows(&[
        ident(Some("15"), "9780316769488"),
        ident(Some("calibre"), "412"),
        ident(Some("uuid"), "c0e51a66-085f-4805-b116-a0d451d281bd"),
    ]);
    assert_eq!(rows.len(), 3);
    assert_eq!(
        rows.iter().map(|r| r.label.as_str()).collect::<Vec<_>>(),
        ["ISBN-13", "Calibre ID", "Source UUID"]
    );
}

#[test]
fn bd_identifier_rows_drop_a_blank_value() {
    let rows = bd_identifier_rows(&[ident(Some("ISBN"), "   "), ident(None, "")]);
    assert!(rows.is_empty());
}

#[test]
fn bd_identifier_rows_give_every_row_a_distinct_key() {
    let rows = bd_identifier_rows(&[
        ident(Some("ISBN"), "111"),
        ident(Some("ISBN"), "222"),
        ident(None, "333"),
    ]);
    let mut keys: Vec<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    keys.sort_unstable();
    let before = keys.len();
    keys.dedup();
    assert_eq!(keys.len(), before);
}
