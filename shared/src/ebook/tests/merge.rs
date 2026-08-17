//! `MetadataOverrides::merge()` tests: override-layer composition
//! semantics — empty-vec-wins, `None` preserves the base layer, scalar
//! passthrough — across creators/subjects/isbn/print_pages/genres.
//!
//! NOTE on contract: the issue describes `merge` as "applies an override
//! layer onto a base `EbookMetadata`, with empty-array meaning clear-all".
//! The actual `MetadataOverrides::merge` merges two `MetadataOverrides`
//! layers (a prior stored override + an incoming edit), not an
//! `EbookMetadata`, and uses `incoming.field.or(self.field)`. The
//! empty-array-clears-the-base-book behaviour lives in
//! `db::metadata_overrides::apply_overrides`, where `Some(vec![])`
//! overwrites the book's list with an empty vec. The tests below assert the
//! ACTUAL `merge` semantics; case (4) documents that for `merge` itself an
//! empty `Some(vec![])` is an *override layer value* that wins over the
//! base layer (it does not collapse to "don't touch"), which is consistent
//! with — and feeds into — the clear-all behaviour at apply time.

use super::super::*;
use super::{contributor, tags};

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

// --- merge() genre semantics --------------------------------------------

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
