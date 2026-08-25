//! Unit tests for the shelves-row pure helpers: the caption meta line, the
//! spoken label that carries the kind the badge glyph can't, and the slab
//! receipt.

use omnibus_shared::{ShelfKind, Visibility};

use super::*;

#[test]
fn shelf_meta_line_pluralizes_and_marks_public_shelves() {
    assert_eq!(shelf_meta_line(1, Visibility::Private), "1 book");
    assert_eq!(shelf_meta_line(0, Visibility::Private), "0 books");
    assert_eq!(
        shelf_meta_line(12, Visibility::Public),
        "12 books \u{00b7} Public"
    );
}

#[test]
fn shelf_aria_label_names_the_kind_the_badge_glyph_only_draws() {
    assert_eq!(
        shelf_aria_label("Space Operas", 12, ShelfKind::Smart, Visibility::Public),
        "Smart shelf Space Operas, 12 books \u{00b7} Public"
    );
    assert_eq!(
        shelf_aria_label("Wishlist", 3, ShelfKind::Wishlist, Visibility::Private),
        "Wishlist Wishlist, 3 books"
    );
    // A hand-picked shelf has no badge, so its label carries no kind word.
    assert_eq!(
        shelf_aria_label("Reread", 1, ShelfKind::Manual, Visibility::Private),
        "Reread, 1 book"
    );
}

#[test]
fn slab_line_reports_whether_the_pick_is_filtering_the_list_below() {
    assert_eq!(slab_line(false), "Shelves \u{2014} showing everything");
    assert_eq!(slab_line(true), "Shelves \u{2014} filtering the list below");
}
