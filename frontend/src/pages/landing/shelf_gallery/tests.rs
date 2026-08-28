//! Unit tests for the shelves-row pure helpers: the caption meta line, the
//! spoken label that carries the kind the badge glyph can't, the owner
//! attribution rule, and the slab receipt.

use omnibus_shared::{ShelfKind, ShelfSummary, Visibility};

use super::*;

fn summary(owner_user_id: i64, kind: ShelfKind) -> ShelfSummary {
    ShelfSummary {
        id: 7,
        owner_user_id,
        owner_username: "elena".into(),
        kind,
        name: "Lunch Break Picks".into(),
        visibility: Visibility::Private,
        accent: None,
        book_count: 0,
        cover_uuids: Vec::new(),
    }
}

#[test]
fn shelf_meta_line_pluralizes_and_marks_public_shelves() {
    assert_eq!(shelf_meta_line(1, Visibility::Private, None), "1 book");
    assert_eq!(shelf_meta_line(0, Visibility::Private, None), "0 books");
    assert_eq!(
        shelf_meta_line(12, Visibility::Public, None),
        "12 books \u{00b7} Public"
    );
}

#[test]
fn shelf_meta_line_attributes_a_shelf_the_viewer_does_not_own() {
    assert_eq!(
        shelf_meta_line(0, Visibility::Private, Some("elena")),
        "0 books \u{00b7} by elena"
    );
    assert_eq!(
        shelf_meta_line(12, Visibility::Public, Some("elena")),
        "12 books \u{00b7} Public \u{00b7} by elena"
    );
}

#[test]
fn attributed_owner_names_only_shelves_the_viewer_does_not_own() {
    let shelf = summary(2, ShelfKind::Manual);
    assert_eq!(attributed_owner(&shelf, Some(1)), Some("elena"));
    // Your own shelf needs no attribution — that is what distinguishes it.
    assert_eq!(attributed_owner(&shelf, Some(2)), None);
    // Viewer unresolved (SSR + first paint): withhold rather than guess.
    assert_eq!(attributed_owner(&shelf, None), None);
    // The Wishlist name already opens with its owner.
    assert_eq!(attributed_owner(&summary(2, ShelfKind::Wishlist), Some(1)), None);
}

#[test]
fn shelf_aria_label_names_the_kind_the_badge_glyph_only_draws() {
    assert_eq!(
        shelf_aria_label("Space Operas", 12, ShelfKind::Smart, Visibility::Public, None),
        "Smart shelf Space Operas, 12 books \u{00b7} Public"
    );
    assert_eq!(
        shelf_aria_label("Wishlist", 3, ShelfKind::Wishlist, Visibility::Private, None),
        "Wishlist Wishlist, 3 books"
    );
    // A hand-picked shelf has no badge, so its label carries no kind word.
    assert_eq!(
        shelf_aria_label("Reread", 1, ShelfKind::Manual, Visibility::Private, None),
        "Reread, 1 book"
    );
}

#[test]
fn shelf_aria_label_speaks_the_owner_of_someone_elses_shelf() {
    assert_eq!(
        shelf_aria_label(
            "Lunch Break Picks",
            0,
            ShelfKind::Manual,
            Visibility::Private,
            Some("elena")
        ),
        "Lunch Break Picks, 0 books \u{00b7} by elena"
    );
}

#[test]
fn slab_line_reports_whether_the_pick_is_filtering_the_list_below() {
    assert_eq!(slab_line(false), "Shelves \u{2014} showing everything");
    assert_eq!(slab_line(true), "Shelves \u{2014} filtering the list below");
}
