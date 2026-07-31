//! Coverage for `create_shelf_modal`'s pure request-construction helpers:
//! `build_create_request`'s Smart vs Manual/Wishlist kind-branch, and
//! `create_label`'s formatting.

use omnibus_shared::{RuleField, RuleOp};

use super::*;

/// A single complete Tag-is-Fantasy draft, the minimal input
/// `build_create_request`'s Smart branch needs to emit a rule.
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
fn build_create_request_encodes_smart_kind_with_rules_and_no_book_uuids() {
    let req = build_create_request(
        ShelfKind::Smart,
        "Cosy Reads".into(),
        Visibility::Private,
        MatchMode::All,
        &[complete_draft()],
        &["some-uuid".to_string()],
    );
    assert_eq!(req.kind, ShelfKind::Smart);
    assert_eq!(req.match_mode, Some(MatchMode::All));
    assert_eq!(
        req.rules,
        vec![ShelfRule {
            field: RuleField::Tag,
            op: RuleOp::Is,
            value: "Fantasy".into(),
        }]
    );
    // Smart shelves never carry a picked book list, even if one is passed in.
    assert!(req.book_uuids.is_empty());
}

#[test]
fn build_create_request_encodes_manual_kind_with_picked_books_and_no_rules() {
    let picked = vec!["uuid-1".to_string(), "uuid-2".to_string()];
    let req = build_create_request(
        ShelfKind::Manual,
        "Hand-picked".into(),
        Visibility::Public,
        MatchMode::All,
        &[complete_draft()],
        &picked,
    );
    assert_eq!(req.kind, ShelfKind::Manual);
    assert_eq!(req.match_mode, None);
    assert!(req.rules.is_empty());
    assert_eq!(req.book_uuids, picked);
}

#[test]
fn build_create_request_folds_wishlist_kind_into_manual() {
    // The kind toggle only offers Smart/Manual, but the request builder is
    // also defensive against a Wishlist value reaching it.
    let req = build_create_request(
        ShelfKind::Wishlist,
        "Wishlist".into(),
        Visibility::Private,
        MatchMode::All,
        &[],
        &[],
    );
    assert_eq!(req.kind, ShelfKind::Manual);
}

#[test]
fn create_label_is_plain_for_smart_shelves() {
    assert_eq!(create_label(ShelfKind::Smart, 4), "Create");
}

#[test]
fn create_label_shows_picked_count_for_manual_shelves() {
    assert_eq!(create_label(ShelfKind::Manual, 3), "Create \u{b7} 3");
}

#[test]
fn create_label_shows_picked_count_for_wishlist_kind_too() {
    assert_eq!(create_label(ShelfKind::Wishlist, 0), "Create \u{b7} 0");
}
