//! Coverage for the edit-shelf modal: SSR render-smoke of the Kobo sync
//! opt-in toggle (prefilled from the shelf, never shown for system shelves),
//! plus `build_update_request`'s save-validation branches. Needs the
//! `server` feature (`dioxus::ssr`).

use omnibus_shared::{RuleField, RuleOp, ShelfRule, Visibility};

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

/// A single complete Tag-is-Fantasy draft — the minimal input a smart shelf
/// needs to encode a non-empty rule set.
fn complete_draft() -> RuleDraft {
    RuleDraft {
        field: RuleField::Tag,
        op: RuleOp::Is,
        value: "Fantasy".into(),
        value2: String::new(),
        unit: "d".into(),
    }
}

#[test]
fn build_update_request_rejects_an_empty_name_before_any_other_check() {
    let err = build_update_request(
        "   ",
        Visibility::Private,
        true,
        false,
        true,
        MatchMode::All,
        &[complete_draft()],
    )
    .unwrap_err();
    assert_eq!(err, "Name is required.");
}

#[test]
fn build_update_request_rejects_a_smart_shelf_with_no_complete_rules() {
    // An incomplete draft (empty value) encodes to no wire rule at all.
    let incomplete = RuleDraft::default();
    let err = build_update_request(
        "Cosy Reads",
        Visibility::Private,
        true,
        false,
        true,
        MatchMode::All,
        &[incomplete],
    )
    .unwrap_err();
    assert_eq!(err, "Add at least one condition.");
}

#[test]
fn build_update_request_accepts_a_manual_shelf_with_no_rules() {
    // Manual shelves skip the rule-set check entirely (`is_smart == false`).
    let req = build_update_request(
        "Cosy Reads",
        Visibility::Public,
        true,
        true,
        false,
        MatchMode::All,
        &[],
    )
    .expect("manual shelves don't need any rules");
    assert_eq!(req.name, Some("Cosy Reads".to_string()));
    assert_eq!(req.visibility, Some(Visibility::Public));
    assert_eq!(req.sync_to_kobo, Some(true));
    assert_eq!(req.rules, None);
}

#[test]
fn build_update_request_accepts_a_smart_shelf_with_a_complete_rule() {
    let req = build_update_request(
        "Cosy Reads",
        Visibility::Private,
        true,
        false,
        true,
        MatchMode::Any,
        &[complete_draft()],
    )
    .expect("a complete draft encodes a rule");
    assert_eq!(req.match_mode, Some(MatchMode::Any));
    assert_eq!(
        req.rules,
        Some(vec![ShelfRule {
            field: RuleField::Tag,
            op: RuleOp::Is,
            value: "Fantasy".into(),
        }])
    );
}
