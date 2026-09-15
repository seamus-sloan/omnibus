//! Coverage for the web shelves index: the filter / group / sort / census
//! rules in `filter`, plus SSR render-smoke of a shelf card — owner
//! attribution and the ledge's three states — under a one-route test router.

use omnibus_shared::{IndexSort, ShelfKind, ShelfSummary, Visibility};

use super::filter::*;

const ME: (i64, &str) = (1, "sloan");
const ALICE: (i64, &str) = (2, "Alice");
const BOB: (i64, &str) = (3, "bob");
const VIEWER: Option<i64> = Some(ME.0);

fn shelf(id: i64, owner: (i64, &str), name: &str, kind: ShelfKind, count: i64) -> ShelfSummary {
    ShelfSummary {
        id,
        owner_user_id: owner.0,
        owner_username: owner.1.into(),
        owner_has_avatar: false,
        kind,
        name: name.into(),
        visibility: Visibility::Public,
        accent: None,
        book_count: count,
        cover_uuids: Vec::new(),
    }
}

/// The viewer and two other readers, each with a wishlist, mixed kinds and counts.
fn library() -> Vec<ShelfSummary> {
    vec![
        shelf(10, BOB, "bob's Wishlist", ShelfKind::Wishlist, 2),
        shelf(11, ALICE, "Sci-fi Picks", ShelfKind::Manual, 8),
        shelf(12, ME, "sloan's Wishlist", ShelfKind::Wishlist, 1),
        shelf(13, ME, "To read", ShelfKind::Manual, 24),
        shelf(14, ME, "Fantasy", ShelfKind::Smart, 12),
        shelf(15, ALICE, "Alice's Wishlist", ShelfKind::Wishlist, 0),
        shelf(16, BOB, "Cosy Crime", ShelfKind::Smart, 5),
    ]
}

fn query() -> ShelfQuery {
    ShelfQuery::default()
}

/// Each group's shelf ids, in display order.
fn ids(groups: &[OwnerGroup]) -> Vec<Vec<i64>> {
    groups
        .iter()
        .map(|g| g.shelves.iter().map(|s| s.id).collect())
        .collect()
}

#[test]
fn group_shelves_leads_with_the_viewer_then_other_owners_by_name() {
    let groups = group_shelves(&library(), VIEWER, &query());
    let owners: Vec<i64> = groups.iter().map(|g| g.owner.id).collect();
    // "Alice" before "bob" — case-insensitive — and the viewer ahead of both.
    assert_eq!(owners, vec![ME.0, ALICE.0, BOB.0]);
    assert!(groups[0].owner.is_viewer);
}

#[test]
fn group_shelves_sorts_each_group_by_name_with_the_wishlist_last() {
    let groups = group_shelves(&library(), VIEWER, &query());
    assert_eq!(
        ids(&groups),
        vec![vec![14, 13, 12], vec![11, 15], vec![16, 10]]
    );
}

#[test]
fn group_shelves_sorts_by_book_count_when_asked_and_still_trails_the_wishlist() {
    let q = ShelfQuery {
        sort: IndexSort::BookCount,
        ..query()
    };
    assert_eq!(
        ids(&group_shelves(&library(), VIEWER, &q))[0],
        vec![13, 14, 12]
    );
}

#[test]
fn group_shelves_narrows_to_one_owner() {
    let q = ShelfQuery {
        owner: OwnerFilter::Owner(ALICE.0),
        ..query()
    };
    assert_eq!(
        ids(&group_shelves(&library(), VIEWER, &q)),
        vec![vec![11, 15]]
    );
}

#[test]
fn group_shelves_narrows_to_one_kind_and_drops_owners_left_empty() {
    let q = ShelfQuery {
        kind: KindFilter::Smart,
        ..query()
    };
    // Alice has no smart shelf, so her whole group drops out.
    assert_eq!(
        ids(&group_shelves(&library(), VIEWER, &q)),
        vec![vec![14], vec![16]]
    );
}

#[test]
fn group_shelves_matches_text_against_shelf_and_owner_names() {
    let by_owner = ShelfQuery {
        text: "ALICE".into(),
        ..query()
    };
    assert_eq!(
        ids(&group_shelves(&library(), VIEWER, &by_owner)),
        vec![vec![11, 15]]
    );
    let by_name = ShelfQuery {
        text: "  crime ".into(),
        ..query()
    };
    assert_eq!(
        ids(&group_shelves(&library(), VIEWER, &by_name)),
        vec![vec![16]]
    );
}

#[test]
fn matches_text_requires_every_term_and_passes_a_blank_filter() {
    let picks = shelf(11, ALICE, "Sci-fi Picks", ShelfKind::Manual, 8);
    assert!(matches_text(&picks, "alice sci"));
    assert!(!matches_text(&picks, "alice crime"));
    assert!(matches_text(&picks, "   "));
}

#[test]
fn owners_counts_each_owners_shelves_and_flags_the_viewer() {
    let summary: Vec<(i64, usize, bool)> = owners(&library(), VIEWER)
        .iter()
        .map(|o| (o.id, o.shelf_count, o.is_viewer))
        .collect();
    assert_eq!(
        summary,
        vec![(ME.0, 3, true), (ALICE.0, 2, false), (BOB.0, 2, false)]
    );
}

#[test]
fn census_splits_the_viewers_shelves_from_other_readers() {
    assert_eq!(
        census(&library(), VIEWER),
        "7 shelves \u{b7} 3 yours \u{b7} 4 from 2 other readers"
    );
}

#[test]
fn census_reports_just_the_count_when_every_shelf_is_the_viewers() {
    let mine: Vec<ShelfSummary> = library()
        .into_iter()
        .filter(|s| s.owner_user_id == ME.0)
        .collect();
    // The group headings below already say whose these are, so the count
    // stands on its own rather than adding "all yours".
    assert_eq!(census(&mine, VIEWER), "3 shelves");
    assert_eq!(census(&mine[..1], VIEWER), "1 shelf");
}

#[test]
fn census_counts_other_readers_when_the_viewer_has_none() {
    let theirs: Vec<ShelfSummary> = library()
        .into_iter()
        .filter(|s| s.owner_user_id == ALICE.0)
        .collect();
    assert_eq!(census(&theirs, VIEWER), "2 shelves from 1 reader");
    assert_eq!(census(&[], VIEWER), "No shelves yet");
}

#[test]
fn shows_others_private_flags_only_someone_elses_private_shelf() {
    let mut shelves = library();
    assert!(!shows_others_private(&shelves, VIEWER));
    shelves[3].visibility = Visibility::Private; // the viewer's own "To read"
    assert!(!shows_others_private(&shelves, VIEWER));
    shelves[1].visibility = Visibility::Private; // Alice's "Sci-fi Picks"
    assert!(shows_others_private(&shelves, VIEWER));
}

#[test]
fn shelf_query_is_filtering_ignores_the_sort_axis_and_blank_text() {
    let sorted = ShelfQuery {
        sort: IndexSort::BookCount,
        ..query()
    };
    assert!(!sorted.is_filtering());
    let blank = ShelfQuery {
        text: "   ".into(),
        ..query()
    };
    assert!(!blank.is_filtering());
    let wishlists = ShelfQuery {
        kind: KindFilter::Wishlist,
        ..query()
    };
    assert!(wishlists.is_filtering());
}

#[test]
fn empty_message_tells_no_shelves_apart_from_no_matches() {
    assert!(empty_message(&query(), false).starts_with("No shelves yet"));
    let text = ShelfQuery {
        text: " zzz ".into(),
        ..query()
    };
    assert_eq!(
        empty_message(&text, true),
        "No shelves match \u{201c}zzz\u{201d}."
    );
    let smart = ShelfQuery {
        kind: KindFilter::Smart,
        ..query()
    };
    assert_eq!(
        empty_message(&smart, true),
        "No shelves match these filters."
    );
}

#[test]
fn group_title_names_the_viewer_as_you_and_others_by_name() {
    let mut owner = owners(&library(), VIEWER).remove(0);
    assert_eq!(group_title(&owner), "Your shelves");
    owner.is_viewer = false;
    assert_eq!(group_title(&owner), "sloan\u{2019}s shelves");
}

#[test]
fn kind_filter_admits_only_its_own_kind() {
    assert!(KindFilter::Any.admits(ShelfKind::Wishlist));
    assert!(KindFilter::Manual.admits(ShelfKind::Manual));
    assert!(!KindFilter::Manual.admits(ShelfKind::Smart));
}

// SSR render-smoke for the card. `ShelfCard` is a router `Link`, which panics
// without a parent router, so each variant gets a one-route test router
// mounted at `/` (see `book_detail/highlights/tests.rs`).
#[cfg(feature = "server")]
mod render {
    use dioxus::prelude::*;
    use dioxus_router::{Routable, Router};
    use omnibus_shared::ShelfKind;

    use super::super::card::ShelfCard;
    use super::{shelf, ALICE, ME};
    use crate::test_support::render_in_vdom;

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum OwnCardRoute {
        #[route("/")]
        OwnCardHost {},
    }

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum OtherCardRoute {
        #[route("/")]
        OtherCardHost {},
    }

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum EmptySmartRoute {
        #[route("/")]
        EmptySmartHost {},
    }

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum CoverlessRoute {
        #[route("/")]
        CoverlessHost {},
    }

    #[component]
    fn OwnCardHost() -> Element {
        let mut to_read = shelf(13, ME, "To read", ShelfKind::Manual, 24);
        to_read.cover_uuids = vec!["u1".into(), "u2".into()];
        rsx! { ShelfCard { shelf: to_read, is_viewer: true, server_url: "", avatar_bust: 0 } }
    }

    #[component]
    fn OtherCardHost() -> Element {
        let picks = shelf(11, ALICE, "Sci-fi Picks", ShelfKind::Manual, 8);
        rsx! { ShelfCard { shelf: picks, is_viewer: false, server_url: "", avatar_bust: 0 } }
    }

    #[component]
    fn EmptySmartHost() -> Element {
        let fresh = shelf(20, ME, "New arrivals", ShelfKind::Smart, 0);
        rsx! { ShelfCard { shelf: fresh, is_viewer: true, server_url: "", avatar_bust: 0 } }
    }

    #[component]
    fn CoverlessHost() -> Element {
        let plain = shelf(21, ME, "Plain", ShelfKind::Manual, 3);
        rsx! { ShelfCard { shelf: plain, is_viewer: true, server_url: "", avatar_bust: 0 } }
    }

    fn own_card() -> Element {
        rsx! { Router::<OwnCardRoute> {} }
    }

    fn other_card() -> Element {
        rsx! { Router::<OtherCardRoute> {} }
    }

    fn empty_smart_card() -> Element {
        rsx! { Router::<EmptySmartRoute> {} }
    }

    fn coverless_card() -> Element {
        rsx! { Router::<CoverlessRoute> {} }
    }

    #[test]
    fn shelf_card_attributes_the_viewers_shelf_to_you_and_links_to_it() {
        let html = render_in_vdom(own_card);
        assert!(html.contains("data-testid=\"shelf-card-13\""));
        assert!(html.contains("href=\"/shelves/13\""));
        assert!(html.contains(">You<"));
        assert!(html.contains("/api/thumbs/u1/sm"));
        assert!(html.contains("24 books"));
    }

    #[test]
    fn shelf_card_names_another_readers_shelf_after_its_owner() {
        let html = render_in_vdom(other_card);
        assert!(html.contains(">Alice<"));
        assert!(!html.contains(">You<"));
        assert!(html.contains("Hand-picked"));
        assert!(html.contains("Public"));
    }

    #[test]
    fn shelf_card_shows_an_empty_ledge_in_the_kinds_own_words() {
        let html = render_in_vdom(empty_smart_card);
        assert!(html.contains("No matches yet"));
        assert!(!html.contains("shv-ledge-book"));
    }

    #[test]
    fn shelf_card_stands_blank_plates_for_members_without_covers() {
        let html = render_in_vdom(coverless_card);
        assert_eq!(html.matches("shv-ledge-book--blank").count(), 3);
        assert!(!html.contains("Empty shelf"));
    }
}
