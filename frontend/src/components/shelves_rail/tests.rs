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

// Render-smoke coverage for the public `ShelvesRail` component itself,
// unlike the helper tests above. Gated on `server` (needs `dioxus::ssr`).
//
// `ShelvesRail` renders a `dioxus_router::Link` for the "All books" row (and
// one per shelf), which panics without a live `RouterContext` — only
// obtainable by mounting `dioxus_router::Router`, which this test doesn't do
// (same constraint noted on `components::top_nav`'s untested full render;
// `book_detail/highlights/tests.rs` mounts a `Router` for `Link` coverage,
// but wiring that harness here is out of scope). Dioxus catches that panic
// per-component rather than aborting the whole render, so the assertions
// below on the surrounding, Link-free markup still hold — the "All books"
// row itself is unverifiable here.
#[cfg(feature = "server")]
mod render {
    use dioxus::prelude::*;

    use crate::components::shelves_rail::{RailActive, ShelvesRail};
    use crate::test_support::render_in_vdom;

    /// A single `rebuild_in_place()` doesn't drive the mount-fetch effect to
    /// completion, so the rail renders in its pre-fetch state: empty shelf
    /// list and the "New shelf" trigger — the SSR/first-paint steady state
    /// (rule 07) before the list arrives.
    fn harness() -> Element {
        rsx! {
            ShelvesRail { active: RailActive::All }
        }
    }

    #[test]
    fn shelves_rail_renders_the_shell_and_new_shelf_trigger() {
        let html = render_in_vdom(harness);
        assert!(html.contains("data-testid=\"shelves-rail\""));
        assert!(html.contains("data-testid=\"new-shelf\""));
        // The create-shelf modal only mounts once "New shelf" is clicked.
        assert!(!html.contains("data-testid=\"create-shelf-modal\""));
    }
}
