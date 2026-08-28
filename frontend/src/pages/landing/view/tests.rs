//! Unit tests for the landing page's pure view derivations: which list feeds
//! the grid, what the section header is titled, and where its count comes
//! from.

use omnibus_shared::{ShelfKind, ShelfSummary, Visibility};

use super::*;

fn shelf(id: i64, name: &str, book_count: i64) -> ShelfSummary {
    ShelfSummary {
        id,
        owner_user_id: 1,
        owner_username: "elena".into(),
        kind: ShelfKind::Manual,
        name: name.into(),
        visibility: Visibility::Private,
        accent: None,
        book_count,
        cover_uuids: Vec::new(),
    }
}

#[test]
fn visible_source_ranks_search_over_a_shelf_pick_over_browse() {
    assert_eq!(
        visible_source(true, ShelfSelection::Shelf(7)),
        VisibleSource::Search
    );
    assert_eq!(
        visible_source(false, ShelfSelection::Shelf(7)),
        VisibleSource::Shelf
    );
    assert_eq!(
        visible_source(false, ShelfSelection::All),
        VisibleSource::Browse
    );
}

#[test]
fn section_title_names_the_pick_and_falls_back_while_the_list_loads() {
    let shelves = vec![shelf(7, "Lunch Break Picks", 3)];
    assert_eq!(section_title(ShelfSelection::All, &shelves), "All Books");
    assert_eq!(
        section_title(ShelfSelection::Shelf(7), &shelves),
        "Lunch Break Picks"
    );
    assert_eq!(section_title(ShelfSelection::Shelf(7), &[]), "Shelf");
}

#[test]
fn shelf_book_count_prefers_the_loaded_member_list_over_the_gallery_summary() {
    // The summary's aggregate is stale after a membership change; the member
    // list is the one that refetched, so the header must follow it.
    let shelves = vec![shelf(7, "Lunch Break Picks", 3)];
    assert_eq!(
        shelf_book_count(ShelfSelection::Shelf(7), &shelves, Some(4)),
        4
    );
    assert_eq!(
        shelf_book_count(ShelfSelection::Shelf(7), &shelves, Some(0)),
        0
    );
}

#[test]
fn shelf_book_count_stands_in_with_the_summary_until_the_members_land() {
    let shelves = vec![shelf(7, "Lunch Break Picks", 3)];
    assert_eq!(
        shelf_book_count(ShelfSelection::Shelf(7), &shelves, None),
        3
    );
    // A pick whose shelf isn't in the (still-loading) list counts nothing
    // rather than guessing.
    assert_eq!(
        shelf_book_count(ShelfSelection::Shelf(9), &shelves, None),
        0
    );
    assert_eq!(shelf_book_count(ShelfSelection::All, &shelves, None), 0);
}
