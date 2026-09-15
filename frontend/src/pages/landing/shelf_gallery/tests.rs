//! Unit tests for the shelves-row pure helpers: which shelves the row carries
//! and in what order, the caption meta line, the spoken label that carries the
//! kind the badge glyph can't, the owner attribution rule, and the slab
//! receipt.

use omnibus_shared::{ShelfKind, ShelfSummary, Visibility};

use super::*;

fn summary(owner_user_id: i64, kind: ShelfKind) -> ShelfSummary {
    ShelfSummary {
        id: 7,
        owner_user_id,
        owner_username: "elena".into(),
        owner_has_avatar: false,
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
    assert_eq!(
        attributed_owner(&summary(2, ShelfKind::Wishlist), Some(1)),
        None
    );
}

#[test]
fn shelf_aria_label_names_the_kind_the_badge_glyph_only_draws() {
    assert_eq!(
        shelf_aria_label(
            "Space Operas",
            12,
            ShelfKind::Smart,
            Visibility::Public,
            None
        ),
        "Smart shelf Space Operas, 12 books \u{00b7} Public"
    );
    assert_eq!(
        shelf_aria_label(
            "Wishlist",
            3,
            ShelfKind::Wishlist,
            Visibility::Private,
            None
        ),
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

// ── Which shelves the row carries (mirrors the iOS rail's `railShelves`) ──

const ME: i64 = 1;
const THEM: i64 = 2;

fn shelf(id: i64, owner: i64, kind: ShelfKind, vis: Visibility, books: i64) -> ShelfSummary {
    ShelfSummary {
        id,
        visibility: vis,
        book_count: books,
        ..summary(owner, kind)
    }
}

fn ids(shelves: &[ShelfSummary]) -> Vec<i64> {
    shelves.iter().map(|s| s.id).collect()
}

#[test]
fn rail_shelves_keeps_the_viewers_own_whatever_their_visibility_or_kind() {
    let all = vec![
        shelf(1, ME, ShelfKind::Manual, Visibility::Private, 0),
        shelf(2, ME, ShelfKind::Smart, Visibility::Public, 4),
    ];
    // An empty shelf of your own still belongs on the row — you made it.
    assert_eq!(ids(&rail_shelves(&all, Some(ME))), vec![1, 2]);
}

#[test]
fn rail_shelves_keeps_another_readers_shared_shelf_but_not_their_private_one() {
    let all = vec![
        shelf(1, THEM, ShelfKind::Manual, Visibility::Public, 3),
        shelf(2, THEM, ShelfKind::Manual, Visibility::Private, 3),
    ];
    // Only an admin's read returns someone else's private shelf at all, and
    // the row is not where it should surface.
    assert_eq!(ids(&rail_shelves(&all, Some(ME))), vec![1]);
}

#[test]
fn rail_shelves_drops_another_readers_wishlist_even_when_stocked() {
    // A wishlist is provisioned per account rather than chosen, so someone
    // else's is noise on your row however full it is.
    let all = vec![shelf(1, THEM, ShelfKind::Wishlist, Visibility::Public, 9)];
    assert!(rail_shelves(&all, Some(ME)).is_empty());
}

#[test]
fn rail_shelves_drops_an_empty_wishlist_and_keeps_a_stocked_one() {
    let empty = vec![shelf(1, ME, ShelfKind::Wishlist, Visibility::Public, 0)];
    assert!(rail_shelves(&empty, Some(ME)).is_empty());
    let stocked = vec![shelf(1, ME, ShelfKind::Wishlist, Visibility::Public, 1)];
    assert_eq!(ids(&rail_shelves(&stocked, Some(ME))), vec![1]);
}

#[test]
fn rail_shelves_counts_nothing_as_the_viewers_until_the_viewer_resolves() {
    let all = vec![
        shelf(1, ME, ShelfKind::Manual, Visibility::Private, 2),
        shelf(2, THEM, ShelfKind::Manual, Visibility::Public, 2),
    ];
    // SSR and first paint: the viewer isn't known, so only a shared shelf
    // qualifies. The viewer's own join once the boot effect lands.
    assert_eq!(ids(&rail_shelves(&all, None)), vec![2]);
    assert_eq!(ids(&rail_shelves(&all, Some(ME))), vec![1, 2]);
}

#[test]
fn rail_shelves_preserves_the_servers_order_and_keeps_every_qualifying_shelf() {
    let all: Vec<ShelfSummary> = (1..=12)
        .map(|id| shelf(id, ME, ShelfKind::Manual, Visibility::Private, 1))
        .collect();
    // No sort and no truncation of its own: the server already ordered these
    // viewer-first, then by creation position, and the row's arrows page
    // through the tail rather than dropping it (where the iOS rail takes 8).
    assert_eq!(
        ids(&rail_shelves(&all, Some(ME))),
        (1..=12).collect::<Vec<_>>()
    );
}
