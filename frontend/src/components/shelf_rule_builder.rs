//! Smart-shelf rule builder — the condition-row editor + live-preview pane
//! shared by shelf creation ([`crate::components::CreateShelfModal`]) and
//! editing ([`crate::components::EditShelfModal`]). [`RuleDraft`] is the
//! per-row editable state; [`RuleBuilder`] is the mounted editor + preview.

use dioxus::prelude::*;
use omnibus_shared::{MatchMode, RuleField, RuleOp, RulePreview, ShelfRule};

use crate::components::{CoverTile, CoverTileKind};
use crate::data;

/// Field/op/date-unit choices exposed in the smart-rule editor.
const FIELDS: &[(RuleField, &str)] = &[
    (RuleField::Tag, "Tag"),
    (RuleField::Author, "Author"),
    (RuleField::Series, "Series"),
    (RuleField::Rating, "Rating"),
    (RuleField::Status, "Reading status"),
    (RuleField::Format, "Format"),
    (RuleField::Year, "Year"),
    (RuleField::DateAdded, "Date added"),
    (RuleField::DateUpdated, "Date updated"),
];

/// The read-status values a `Status` condition can match, in the order the
/// dropdown offers them. First entry is the default a newly-picked `Status`
/// field snaps to.
const STATUS_VALUES: &[(&str, &str)] = &[
    ("finished", "finished"),
    ("reading", "reading"),
    ("unread", "unread"),
];

const OPS: &[(RuleOp, &str)] = &[
    (RuleOp::Is, "is"),
    (RuleOp::IsNot, "is not"),
    (RuleOp::Contains, "contains"),
    (RuleOp::StartsWith, "starts with"),
    (RuleOp::Gte, "is at least"),
    (RuleOp::Includes, "includes"),
    (RuleOp::InLast, "in the last"),
    (RuleOp::Between, "between"),
    (RuleOp::Before, "before"),
    (RuleOp::After, "after"),
];

/// A rule row's working state. `value`/`value2`/`unit` are the raw input
/// strings; [`RuleDraft::encode_value`] folds them into the wire
/// `ShelfRule.value`, and [`RuleDraft::from_rule`] reverses that fold to
/// prefill an editor from a saved rule.
#[derive(Clone, PartialEq)]
pub struct RuleDraft {
    pub field: RuleField,
    pub op: RuleOp,
    pub value: String,
    /// Second date for `Between`.
    pub value2: String,
    /// Relative-window unit (`d`/`w`/`m`/`y`) for `InLast`.
    pub unit: String,
}

impl Default for RuleDraft {
    fn default() -> Self {
        RuleDraft {
            field: RuleField::Tag,
            op: RuleOp::Is,
            value: String::new(),
            value2: String::new(),
            unit: "d".into(),
        }
    }
}

impl RuleDraft {
    /// Fold the raw inputs into the wire `value` string per the field's op.
    fn encode_value(&self) -> String {
        match self.op {
            RuleOp::InLast => format!("{}{}", self.value.trim(), self.unit),
            RuleOp::Between => format!("{}..{}", self.value.trim(), self.value2.trim()),
            _ => self.value.trim().to_string(),
        }
    }

    /// `true` once every input the current op needs is filled in.
    fn is_complete(&self) -> bool {
        match self.op {
            RuleOp::InLast => !self.value.trim().is_empty(),
            RuleOp::Between => !self.value.trim().is_empty() && !self.value2.trim().is_empty(),
            _ => !self.value.trim().is_empty(),
        }
    }

    /// Convert to a wire rule when complete.
    pub fn to_rule(&self) -> Option<ShelfRule> {
        if !self.is_complete() {
            return None;
        }
        Some(ShelfRule {
            field: self.field,
            op: self.op,
            value: self.encode_value(),
        })
    }

    /// Reconstruct an editable draft from a saved rule (edit-mode prefill),
    /// reversing [`RuleDraft::encode_value`] per op.
    pub fn from_rule(rule: &ShelfRule) -> Self {
        let (value, value2, unit) = match rule.op {
            RuleOp::InLast => {
                let raw = rule.value.as_str();
                let split = raw.len().saturating_sub(1);
                let (num, unit) = raw.split_at(split);
                (num.to_string(), String::new(), unit.to_string())
            }
            RuleOp::Between => {
                let (start, end) = rule.value.split_once("..").unwrap_or((&rule.value, ""));
                (start.to_string(), end.to_string(), "d".to_string())
            }
            _ => (rule.value.clone(), String::new(), "d".to_string()),
        };
        RuleDraft {
            field: rule.field,
            op: rule.op,
            value,
            value2,
            unit,
        }
    }
}

/// Match-mode row + condition-row editor + live-preview pane. Shared between
/// the create modal and [`crate::components::EditShelfModal`] — both own
/// the `rules`/`match_mode` state and hand this component a signal to edit.
#[component]
pub fn RuleBuilder(
    match_mode: MatchMode,
    rules: Signal<Vec<RuleDraft>>,
    on_match_mode: EventHandler<MatchMode>,
    server_url: String,
) -> Element {
    let mut rules = rules;
    let preview = use_signal(|| None::<RulePreview>);

    // Recompute the preview whenever the rule set or match mode changes. Keyed
    // on a memo of the encoded rules so unrelated re-renders don't refetch.
    let preview_key = use_memo(move || {
        let complete: Vec<ShelfRule> = rules.read().iter().filter_map(RuleDraft::to_rule).collect();
        (match_mode, complete)
    });
    let preview_url = server_url.clone();
    use_effect(move || {
        let (mode, wire) = preview_key();
        let url = preview_url.clone();
        let mut preview = preview;
        if wire.is_empty() {
            preview.set(None);
            return;
        }
        spawn(async move {
            if let Ok(p) = data::preview_shelf_rule(&url, mode, wire).await {
                preview.set(Some(p));
            }
        });
    });

    rsx! {
        div { class: "shelf-smart",
            div { class: "shelf-smart-editor",
                div { class: "shelf-match-row",
                    span { "Match" }
                    select {
                        class: "shelf-select",
                        "data-testid": "shelf-match-mode",
                        value: match_mode.as_str(),
                        onchange: move |e| {
                            if let Some(m) = MatchMode::from_str(&e.value()) {
                                on_match_mode.call(m);
                            }
                        },
                        option { value: "any", "any" }
                        option { value: "all", "all" }
                    }
                    span { "of these conditions" }
                }

                for (i, draft) in rules.read().iter().cloned().enumerate() {
                    ConditionRow {
                        key: "{i}",
                        index: i,
                        draft: draft,
                        can_remove: rules.read().len() > 1,
                        on_change: move |d: RuleDraft| {
                            rules.write()[i] = d;
                        },
                        on_remove: move |_| {
                            rules.write().remove(i);
                        },
                    }
                }

                button {
                    r#type: "button",
                    class: "shelf-add-condition",
                    "data-testid": "add-condition",
                    onclick: move |_| rules.write().push(RuleDraft::default()),
                    "\u{FF0B} Add condition"
                }
            }

            div { class: "shelf-preview",
                {render_preview(&preview.read(), &server_url)}
            }
        }
    }
}

/// The live-preview pane content: count + sample covers.
fn render_preview(preview: &Option<RulePreview>, server_url: &str) -> Element {
    match preview {
        None => rsx! {
            p { class: "shelf-preview-empty mono", "Add a condition to preview matches." }
        },
        Some(p) => rsx! {
            p {
                class: "shelf-preview-count",
                "data-testid": "rule-preview-count",
                em { "{p.matched}" }
                " of {p.total} match"
            }
            div { class: "shelf-preview-grid",
                for book in p.sample.iter().cloned() {
                    div {
                        key: "{book.id}",
                        CoverTile {
                            book,
                            server_url: server_url.to_string(),
                            sizes: "120px".to_string(),
                            kind: CoverTileKind::ReadOnly,
                        }
                    }
                }
            }
        },
    }
}

/// The five per-row `FormEvent` handlers a [`ConditionRow`] wires up. `Copy`
/// (Dioxus event handlers), so passing the whole set around — into the rsx
/// and into [`condition_value_input`] — stays a single value.
#[derive(Clone, Copy)]
struct ConditionHandlers {
    on_field: EventHandler<FormEvent>,
    on_op: EventHandler<FormEvent>,
    on_val: EventHandler<FormEvent>,
    on_val2: EventHandler<FormEvent>,
    on_unit: EventHandler<FormEvent>,
}

/// Builds the field/op/value(s)/unit change handlers for one condition row.
/// Each clones the row's current draft, applies its one field, and reports
/// the result through the shared `on_change`. Split out of [`ConditionRow`]
/// to keep it under the line cap.
fn build_condition_handlers(
    draft: RuleDraft,
    on_change: EventHandler<RuleDraft>,
) -> ConditionHandlers {
    // On a field change, snap the op to the first one the new field accepts.
    let d_field = draft.clone();
    let on_field = EventHandler::new(move |e: FormEvent| {
        if let Some(f) = RuleField::from_str(&e.value()) {
            let mut d = d_field.clone();
            d.field = f;
            if !f.accepts(d.op) {
                d.op = OPS
                    .iter()
                    .map(|(o, _)| *o)
                    .find(|o| f.accepts(*o))
                    .unwrap_or(RuleOp::Is);
            }
            // `Status` uses a fixed dropdown; seed a valid default so the row is
            // complete without the user having to open the select.
            if f == RuleField::Status
                && omnibus_shared::ReadStatus::from_str(d.value.trim()).is_none()
            {
                d.value = STATUS_VALUES[0].0.to_string();
            }
            on_change.call(d);
        }
    });
    let d_op = draft.clone();
    let on_op = EventHandler::new(move |e: FormEvent| {
        if let Some(o) = RuleOp::from_str(&e.value()) {
            let mut d = d_op.clone();
            d.op = o;
            on_change.call(d);
        }
    });
    let d_val = draft.clone();
    let on_val = EventHandler::new(move |e: FormEvent| {
        let mut d = d_val.clone();
        d.value = e.value();
        on_change.call(d);
    });
    let d_val2 = draft.clone();
    let on_val2 = EventHandler::new(move |e: FormEvent| {
        let mut d = d_val2.clone();
        d.value2 = e.value();
        on_change.call(d);
    });
    let d_unit = draft.clone();
    let on_unit = EventHandler::new(move |e: FormEvent| {
        let mut d = d_unit.clone();
        d.unit = e.value();
        on_change.call(d);
    });

    ConditionHandlers {
        on_field,
        on_op,
        on_val,
        on_val2,
        on_unit,
    }
}

/// One editable smart-rule condition row.
#[component]
fn ConditionRow(
    index: usize,
    draft: RuleDraft,
    can_remove: bool,
    on_change: EventHandler<RuleDraft>,
    on_remove: EventHandler<()>,
) -> Element {
    let field = draft.field;
    let op = draft.op;
    let handlers = build_condition_handlers(draft.clone(), on_change);

    rsx! {
        div { class: "shelf-condition", "data-testid": "condition-row-{index}",
            select {
                class: "shelf-select",
                "data-testid": "condition-field-{index}",
                value: field.as_str(),
                onchange: move |e| handlers.on_field.call(e),
                for (f, label) in FIELDS.iter() {
                    option { key: "{f.as_str()}", value: f.as_str(), "{label}" }
                }
            }
            select {
                class: "shelf-select",
                "data-testid": "condition-op-{index}",
                value: op.as_str(),
                onchange: move |e| handlers.on_op.call(e),
                for (o, label) in OPS.iter().filter(|(o, _)| field.accepts(*o)) {
                    option { key: "{o.as_str()}", value: o.as_str(), "{label}" }
                }
            }
            {condition_value_input(&draft, handlers)}
            if can_remove {
                button {
                    r#type: "button",
                    class: "shelf-condition-remove",
                    "data-testid": "condition-remove-{index}",
                    "aria-label": "Remove condition",
                    onclick: move |_| on_remove.call(()),
                    "\u{00D7}"
                }
            }
        }
    }
}

/// Field-aware value input(s) for a condition row.
fn condition_value_input(draft: &RuleDraft, handlers: ConditionHandlers) -> Element {
    let value = draft.value.clone();
    let value2 = draft.value2.clone();
    let unit = draft.unit.clone();
    let on_val = move |e| handlers.on_val.call(e);
    let on_val2 = move |e| handlers.on_val2.call(e);
    let on_unit = move |e| handlers.on_unit.call(e);

    // Date fields carry op-specific inputs.
    if draft.field.is_date() {
        return match draft.op {
            RuleOp::InLast => rsx! {
                div { class: "shelf-value-group",
                    input {
                        r#type: "number", min: "1", class: "shelf-value shelf-value--num",
                        placeholder: "30", value: "{value}", oninput: on_val,
                    }
                    select { class: "shelf-select", value: "{unit}", onchange: on_unit,
                        option { value: "d", "days" }
                        option { value: "w", "weeks" }
                        option { value: "m", "months" }
                        option { value: "y", "years" }
                    }
                }
            },
            RuleOp::Between => rsx! {
                div { class: "shelf-value-group",
                    input { r#type: "date", class: "shelf-value", value: "{value}", oninput: on_val }
                    span { class: "shelf-value-sep", "to" }
                    input { r#type: "date", class: "shelf-value", value: "{value2}", oninput: on_val2 }
                }
            },
            _ => rsx! {
                input { r#type: "date", class: "shelf-value", value: "{value}", oninput: on_val }
            },
        };
    }

    // Status is a closed set — a dropdown, not free text.
    if matches!(draft.field, RuleField::Status) {
        let selected = if value.is_empty() {
            STATUS_VALUES[0].0.to_string()
        } else {
            value.clone()
        };
        return rsx! {
            select { class: "shelf-select", value: "{selected}", onchange: on_val,
                for (v, label) in STATUS_VALUES.iter() {
                    option { key: "{v}", value: *v, "{label}" }
                }
            }
        };
    }

    // Rating / Year are numeric; the rest are free text matched by name
    // (case-insensitive), so author/series take the name the user sees.
    let (input_type, placeholder) = match draft.field {
        RuleField::Rating => ("number", "4"),
        RuleField::Year => ("number", "2024"),
        RuleField::Author => ("text", "author name"),
        RuleField::Series => ("text", "series name"),
        _ => ("text", "value"),
    };
    let step = if matches!(draft.field, RuleField::Rating) {
        "0.5"
    } else {
        "1"
    };
    rsx! {
        input {
            r#type: "{input_type}",
            step: "{step}",
            class: "shelf-value",
            placeholder: "{placeholder}",
            value: "{value}",
            oninput: on_val,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_rule_decodes_in_last_into_value_and_unit() {
        let rule = ShelfRule {
            field: RuleField::DateAdded,
            op: RuleOp::InLast,
            value: "30d".into(),
        };
        let draft = RuleDraft::from_rule(&rule);
        assert_eq!(draft.value, "30");
        assert_eq!(draft.unit, "d");
        assert_eq!(draft.to_rule(), Some(rule));
    }

    #[test]
    fn from_rule_decodes_between_into_value_and_value2() {
        let rule = ShelfRule {
            field: RuleField::DateAdded,
            op: RuleOp::Between,
            value: "2025-01-01..2025-02-01".into(),
        };
        let draft = RuleDraft::from_rule(&rule);
        assert_eq!(draft.value, "2025-01-01");
        assert_eq!(draft.value2, "2025-02-01");
        assert_eq!(draft.to_rule(), Some(rule));
    }

    #[test]
    fn from_rule_roundtrips_a_plain_value() {
        let rule = ShelfRule {
            field: RuleField::Tag,
            op: RuleOp::Is,
            value: "Fantasy".into(),
        };
        let draft = RuleDraft::from_rule(&rule);
        assert_eq!(draft.value, "Fantasy");
        assert_eq!(draft.to_rule(), Some(rule));
    }
}
