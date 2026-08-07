//! Facet row for a shelf: kind ("Smart shelf" / "Hand-picked shelf" /
//! "Wishlist"), visibility, and — for smart shelves — one chip per rule
//! ("Author is Brandon Sanderson"). Mounted under the shelf-detail title and
//! the landing header's section title; extra badges (Kobo sync, owner
//! attribution) pass through as children so they share the row.

use dioxus::prelude::*;
use omnibus_shared::{RuleField, RuleOp, Shelf, ShelfKind, ShelfRule, Visibility};

/// The facet row. `children` render between the visibility facet and the
/// rule chips, inside the same flex row.
#[component]
pub fn ShelfFacets(shelf: Shelf, children: Element) -> Element {
    let kind_label = match shelf.kind {
        ShelfKind::Smart => "Smart shelf",
        ShelfKind::Manual => "Hand-picked shelf",
        ShelfKind::Wishlist => "Wishlist",
    };
    let vis_label = match shelf.visibility {
        Visibility::Private => "Private",
        Visibility::Public => "Public",
    };
    let is_smart = shelf.kind == ShelfKind::Smart;
    rsx! {
        div { class: "shelf-facets", "data-testid": "shelf-facets",
            span { class: "shelf-facet shelf-facet--kind", "{kind_label}" }
            span { class: "shelf-facet-dot", "\u{00b7}" }
            span { class: "shelf-facet", "{vis_label}" }
            {children}
            if is_smart {
                for (i, rule) in shelf.rules.iter().enumerate() {
                    span { key: "{i}", class: "shelf-rule-chip", "{rule_text(rule)}" }
                }
            }
        }
    }
}

/// Pencil icon for the edit-shelf buttons that sit beside shelf titles.
pub fn pencil_glyph() -> Element {
    rsx! {
        svg {
            width: "15", height: "15", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M17 3a2.828 2.828 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5L17 3z" }
        }
    }
}

/// Human-readable summary of one smart rule, e.g. "Tag is Fantasy".
pub fn rule_text(rule: &ShelfRule) -> String {
    format!(
        "{} {} {}",
        field_label(rule.field),
        op_label(rule.op),
        rule.value
    )
}

/// Display label for a rule field.
fn field_label(field: RuleField) -> &'static str {
    match field {
        RuleField::Tag => "Tag",
        RuleField::Genre => "Genre",
        RuleField::Author => "Author",
        RuleField::Series => "Series",
        RuleField::Rating => "Rating",
        RuleField::Status => "Reading status",
        RuleField::Format => "Format",
        RuleField::Year => "Year",
        RuleField::DateAdded => "Date added",
        RuleField::DateUpdated => "Date updated",
    }
}

/// Display label for a rule operator.
fn op_label(op: RuleOp) -> &'static str {
    match op {
        RuleOp::Is => "is",
        RuleOp::IsNot => "is not",
        RuleOp::Contains => "contains",
        RuleOp::StartsWith => "starts with",
        RuleOp::Gte => "is at least",
        RuleOp::Includes => "includes",
        RuleOp::InLast => "in the last",
        RuleOp::Between => "between",
        RuleOp::Before => "before",
        RuleOp::After => "after",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rule_text_reads_field_op_value() {
        let rule = ShelfRule {
            field: RuleField::Tag,
            op: RuleOp::Is,
            value: "Fantasy".into(),
        };
        assert_eq!(rule_text(&rule), "Tag is Fantasy");
    }
}
