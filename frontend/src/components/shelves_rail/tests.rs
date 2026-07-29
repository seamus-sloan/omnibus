//! Coverage for `render_shelf_row`'s owner-attribution and active-row
//! branching, exercised via the pure helpers it delegates to
//! (`shows_owner_attribution`, `is_row_active`) rather than a full render:
//! `render_shelf_row` mounts a router `Link`, which needs a live Dioxus
//! runtime + router context that a bare SSR string render doesn't provide.

use omnibus_shared::ShelfKind;

use super::*;

#[test]
fn shows_owner_attribution_is_false_for_the_viewers_own_shelf() {
    assert!(!shows_owner_attribution(Some(1), 1, ShelfKind::Manual));
}

#[test]
fn shows_owner_attribution_is_true_for_a_shelf_the_viewer_does_not_own() {
    assert!(shows_owner_attribution(Some(1), 2, ShelfKind::Manual));
}

#[test]
fn shows_owner_attribution_is_false_for_a_wishlist_even_when_not_owned() {
    // A wishlist's name already opens with the owner, so the chip would repeat it.
    assert!(!shows_owner_attribution(Some(1), 2, ShelfKind::Wishlist));
}

#[test]
fn shows_owner_attribution_is_false_while_the_viewer_is_still_unknown() {
    // `viewer_id` is `None` until the boot effect resolves (SSR + first
    // paint); attribution must not show up before we know who's looking.
    assert!(!shows_owner_attribution(None, 2, ShelfKind::Manual));
}

#[test]
fn is_row_active_is_true_when_the_active_shelf_matches_this_row() {
    assert!(is_row_active(RailActive::Shelf(7), 7));
}

#[test]
fn is_row_active_is_false_for_a_different_shelf_id() {
    assert!(!is_row_active(RailActive::Shelf(99), 7));
}

#[test]
fn is_row_active_is_false_when_the_all_books_row_is_active() {
    assert!(!is_row_active(RailActive::All, 7));
}
