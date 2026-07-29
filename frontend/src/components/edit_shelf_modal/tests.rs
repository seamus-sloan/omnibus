//! SSR render-smoke coverage for the edit-shelf modal: the Kobo sync opt-in
//! toggle renders for owner-editable shelves (prefilled from the shelf), and
//! never for system shelves. Needs the `server` feature (`dioxus::ssr`).

use omnibus_shared::Visibility;

use super::*;
use crate::test_support::render;

/// A minimal shelf for prop-driven render tests.
fn shelf(kind: ShelfKind, sync_to_kobo: bool) -> Shelf {
    Shelf {
        id: 7,
        owner_user_id: 1,
        owner_username: "sloan".into(),
        kind,
        name: "Cosy Reads".into(),
        description: None,
        visibility: Visibility::Private,
        accent: None,
        match_mode: (kind == ShelfKind::Smart).then_some(MatchMode::All),
        rules: Vec::new(),
        book_count: 0,
        sync_to_kobo,
    }
}

#[component]
fn Harness(kind: ShelfKind, sync_to_kobo: bool) -> Element {
    rsx! {
        EditShelfModal {
            shelf: shelf(kind, sync_to_kobo),
            on_close: move |_| {},
            on_saved: move |_| {},
        }
    }
}

/// The opening tag holding `data-testid="<testid>"` — attribute-order-proof
/// so assertions survive renderer reordering.
fn tag_with_testid<'a>(html: &'a str, testid: &str) -> &'a str {
    let needle = format!("data-testid=\"{testid}\"");
    let pos = html
        .find(&needle)
        .unwrap_or_else(|| panic!("no element with testid {testid} in: {html}"));
    let start = html[..pos].rfind('<').expect("testid outside a tag");
    let end = pos + html[pos..].find('>').expect("unterminated tag");
    &html[start..=end]
}

#[test]
fn edit_shelf_modal_renders_the_kobo_toggle_off_when_the_shelf_does_not_sync() {
    let html = render(rsx! { Harness { kind: ShelfKind::Manual, sync_to_kobo: false } });
    assert!(html.contains("Sync to Kobo"));
    assert!(tag_with_testid(&html, "edit-shelf-kobo-off").contains("aria-pressed=\"true\""));
    assert!(tag_with_testid(&html, "edit-shelf-kobo-on").contains("aria-pressed=\"false\""));
}

#[test]
fn edit_shelf_modal_prefills_the_kobo_toggle_on_from_the_shelf() {
    let html = render(rsx! { Harness { kind: ShelfKind::Smart, sync_to_kobo: true } });
    assert!(tag_with_testid(&html, "edit-shelf-kobo-on").contains("aria-pressed=\"true\""));
    assert!(tag_with_testid(&html, "edit-shelf-kobo-off").contains("aria-pressed=\"false\""));
}

#[test]
fn edit_shelf_modal_hides_the_kobo_toggle_for_system_shelves() {
    let html = render(rsx! { Harness { kind: ShelfKind::Wishlist, sync_to_kobo: false } });
    assert!(!html.contains("Sync to Kobo"));
    assert!(!html.contains("edit-shelf-kobo-on"));
}
