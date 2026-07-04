//! Create-shelf modal — the two-body form for smart and hand-picked shelves.
//!
//! Smart: a `match [any|all]` + condition-row editor with a live preview pane.
//! Hand-picked: a searchable cover grid whose selection becomes the shelf's
//! initial book list. Both bodies share the header (name + kind + visibility)
//! and submit through [`crate::data::create_shelf`].

use dioxus::prelude::*;
use omnibus_shared::{
    CreateShelfRequest, EbookMetadata, MatchMode, RuleField, RuleOp, RulePreview, Shelf, ShelfKind,
    ShelfRule, Visibility,
};

use crate::components::atrium::Cover;
use crate::{data, use_server_url};

/// Field/op/date-unit choices exposed in the smart-rule editor. `Status` is
/// deliberately omitted — unsupported in v1.
const FIELDS: &[(RuleField, &str)] = &[
    (RuleField::Tag, "Tag"),
    (RuleField::Author, "Author"),
    (RuleField::Series, "Series"),
    (RuleField::Rating, "Rating"),
    (RuleField::Format, "Format"),
    (RuleField::Year, "Year"),
    (RuleField::DateAdded, "Date added"),
    (RuleField::DateUpdated, "Date updated"),
];

const OPS: &[(RuleOp, &str)] = &[
    (RuleOp::Is, "is"),
    (RuleOp::IsNot, "is not"),
    (RuleOp::Gte, "is at least"),
    (RuleOp::Includes, "includes"),
    (RuleOp::InLast, "in the last"),
    (RuleOp::Between, "between"),
    (RuleOp::Before, "before"),
    (RuleOp::After, "after"),
];

/// A rule row's working state. `value`/`value2`/`unit` are the raw input
/// strings; [`encode_value`] folds them into the wire `ShelfRule.value`.
#[derive(Clone, PartialEq)]
struct RuleDraft {
    field: RuleField,
    op: RuleOp,
    value: String,
    /// Second date for `Between`.
    value2: String,
    /// Relative-window unit (`d`/`w`/`m`/`y`) for `InLast`.
    unit: String,
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
    fn to_rule(&self) -> Option<ShelfRule> {
        if !self.is_complete() {
            return None;
        }
        Some(ShelfRule {
            field: self.field,
            op: self.op,
            value: self.encode_value(),
        })
    }
}

/// Create-shelf modal. Emits the created shelf via `on_created`.
#[component]
pub fn CreateShelfModal(on_close: EventHandler<()>, on_created: EventHandler<Shelf>) -> Element {
    let server_url = use_server_url();
    let mut name = use_signal(String::new);
    let mut kind = use_signal(|| ShelfKind::Smart);
    let mut visibility = use_signal(|| Visibility::Private);
    let mut error = use_signal(|| None::<String>);
    let mut saving = use_signal(|| false);

    // Smart-body state.
    let mut match_mode = use_signal(|| MatchMode::All);
    let rules = use_signal(|| vec![RuleDraft::default()]);

    // Hand-picked state.
    let picked = use_signal(Vec::<String>::new);

    let submit_url = server_url.clone();
    let on_submit = move |_| {
        if saving() {
            return;
        }
        let req = match kind() {
            ShelfKind::Smart => {
                let wire: Vec<ShelfRule> =
                    rules.read().iter().filter_map(|r| r.to_rule()).collect();
                CreateShelfRequest {
                    kind: ShelfKind::Smart,
                    name: name(),
                    description: None,
                    visibility: visibility(),
                    match_mode: Some(match_mode()),
                    rules: wire,
                    book_uuids: Vec::new(),
                }
            }
            ShelfKind::Manual => CreateShelfRequest {
                kind: ShelfKind::Manual,
                name: name(),
                description: None,
                visibility: visibility(),
                match_mode: None,
                rules: Vec::new(),
                book_uuids: picked.read().clone(),
            },
        };
        let url = submit_url.clone();
        let on_created = on_created;
        saving.set(true);
        error.set(None);
        spawn(async move {
            match data::create_shelf(&url, req).await {
                Ok(shelf) => on_created.call(shelf),
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let picked_count = picked.read().len();
    let create_label = match kind() {
        ShelfKind::Manual => format!("Create \u{b7} {picked_count}"),
        ShelfKind::Smart => "Create".to_string(),
    };

    rsx! {
        div {
            class: "shelf-modal-overlay",
            "data-testid": "create-shelf-modal",
            onclick: move |_| on_close.call(()),
            div {
                class: "shelf-modal-card",
                onclick: move |e| e.stop_propagation(),

                div { class: "shelf-modal-head",
                    input {
                        r#type: "text",
                        class: "shelf-name-input",
                        placeholder: "Shelf name\u{2026}",
                        "data-testid": "shelf-name-input",
                        value: "{name}",
                        oninput: move |e| name.set(e.value()),
                    }
                    div { class: "shelf-kind-toggle",
                        button {
                            r#type: "button",
                            class: "shelf-toggle-btn",
                            "aria-pressed": if kind() == ShelfKind::Smart { "true" } else { "false" },
                            "data-testid": "shelf-kind-smart",
                            onclick: move |_| kind.set(ShelfKind::Smart),
                            "Smart"
                        }
                        button {
                            r#type: "button",
                            class: "shelf-toggle-btn",
                            "aria-pressed": if kind() == ShelfKind::Manual { "true" } else { "false" },
                            "data-testid": "shelf-kind-manual",
                            onclick: move |_| kind.set(ShelfKind::Manual),
                            "Hand-picked"
                        }
                    }
                    VisibilityToggle { visibility: visibility(), on_change: move |v| visibility.set(v) }
                }

                div { class: "shelf-modal-body",
                    match kind() {
                        ShelfKind::Smart => rsx! {
                            SmartBody {
                                match_mode: match_mode(),
                                rules,
                                on_match_mode: move |m| match_mode.set(m),
                                server_url: server_url.clone(),
                            }
                        },
                        ShelfKind::Manual => rsx! {
                            PickerBody { picked, server_url: server_url.clone() }
                        },
                    }
                }

                if let Some(msg) = error() {
                    p {
                        role: "alert",
                        class: "shelf-modal-error",
                        "data-testid": "shelf-create-error",
                        "{msg}"
                    }
                }

                div { class: "shelf-modal-foot",
                    button {
                        r#type: "button",
                        class: "btn shelf-btn-ghost",
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        r#type: "button",
                        class: "btn shelf-btn-primary",
                        "data-testid": "shelf-create-submit",
                        disabled: saving(),
                        onclick: on_submit,
                        if saving() { "Creating\u{2026}" } else { "{create_label}" }
                    }
                }
            }
        }
    }
}

/// Private/Public segmented control shared by the modal header.
#[component]
fn VisibilityToggle(visibility: Visibility, on_change: EventHandler<Visibility>) -> Element {
    rsx! {
        div { class: "shelf-vis-toggle",
            button {
                r#type: "button",
                class: "shelf-toggle-btn",
                "aria-pressed": if visibility == Visibility::Private { "true" } else { "false" },
                "data-testid": "shelf-vis-private",
                onclick: move |_| on_change.call(Visibility::Private),
                "Private"
            }
            button {
                r#type: "button",
                class: "shelf-toggle-btn",
                "aria-pressed": if visibility == Visibility::Public { "true" } else { "false" },
                "data-testid": "shelf-vis-public",
                onclick: move |_| on_change.call(Visibility::Public),
                "Public"
            }
        }
    }
}

/// Smart-rule editor + live-preview pane.
#[component]
fn SmartBody(
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
        let complete: Vec<ShelfRule> = rules.read().iter().filter_map(|r| r.to_rule()).collect();
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
                    {sample_cover(book, server_url)}
                }
            }
        },
    }
}

/// One preview / picker cover tile (read-only).
fn sample_cover(book: EbookMetadata, server_url: &str) -> Element {
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let (src, srcset) = thumb_srcs(&book, &uuid, server_url);
    rsx! {
        div { class: "shelf-cover-tile",
            Cover {
                book,
                src_override: src,
                srcset,
                sizes: Some("120px".to_string()),
            }
        }
    }
}

/// Build the responsive thumbnail `src`/`srcset` for a book (mirrors GridTile).
fn thumb_srcs(
    book: &EbookMetadata,
    uuid: &str,
    server_url: &str,
) -> (Option<String>, Option<String>) {
    if book.cover_url.is_some() {
        let sm = crate::thumb_url(server_url, uuid, "sm");
        let md = crate::thumb_url(server_url, uuid, "md");
        (Some(sm.clone()), Some(format!("{sm} 160w, {md} 320w")))
    } else {
        (None, None)
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

    // On a field change, snap the op to the first one the new field accepts.
    let d_field = draft.clone();
    let on_field = move |e: FormEvent| {
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
            on_change.call(d);
        }
    };
    let d_op = draft.clone();
    let on_op = move |e: FormEvent| {
        if let Some(o) = RuleOp::from_str(&e.value()) {
            let mut d = d_op.clone();
            d.op = o;
            on_change.call(d);
        }
    };
    let d_val = draft.clone();
    let on_val = move |e: FormEvent| {
        let mut d = d_val.clone();
        d.value = e.value();
        on_change.call(d);
    };
    let d_val2 = draft.clone();
    let on_val2 = move |e: FormEvent| {
        let mut d = d_val2.clone();
        d.value2 = e.value();
        on_change.call(d);
    };
    let d_unit = draft.clone();
    let on_unit = move |e: FormEvent| {
        let mut d = d_unit.clone();
        d.unit = e.value();
        on_change.call(d);
    };

    rsx! {
        div { class: "shelf-condition", "data-testid": "condition-row-{index}",
            select {
                class: "shelf-select",
                "data-testid": "condition-field-{index}",
                value: field.as_str(),
                onchange: on_field,
                for (f, label) in FIELDS.iter() {
                    option { value: f.as_str(), "{label}" }
                }
            }
            select {
                class: "shelf-select",
                "data-testid": "condition-op-{index}",
                value: op.as_str(),
                onchange: on_op,
                for (o, label) in OPS.iter().filter(|(o, _)| field.accepts(*o)) {
                    option { value: o.as_str(), "{label}" }
                }
            }
            {condition_value_input(&draft, on_val, on_val2, on_unit)}
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
fn condition_value_input(
    draft: &RuleDraft,
    on_val: impl FnMut(FormEvent) + 'static,
    on_val2: impl FnMut(FormEvent) + 'static,
    on_unit: impl FnMut(FormEvent) + 'static,
) -> Element {
    let value = draft.value.clone();
    let value2 = draft.value2.clone();
    let unit = draft.unit.clone();

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

    // Rating / Year are numeric; the rest are free text (author/series accept
    // a numeric id in v1).
    let (input_type, placeholder) = match draft.field {
        RuleField::Rating => ("number", "4"),
        RuleField::Year => ("number", "2024"),
        RuleField::Author | RuleField::Series => ("number", "id"),
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

/// Hand-picked picker: a searchable, selectable cover grid over the library.
#[component]
fn PickerBody(picked: Signal<Vec<String>>, server_url: String) -> Element {
    let mut picked = picked;
    let mut library = use_signal(Vec::<EbookMetadata>::new);
    let mut query = use_signal(String::new);

    let effect_url = server_url.clone();
    use_effect(move || {
        let url = effect_url.clone();
        spawn(async move {
            if let Ok(lib) = data::get_ebooks(&url).await {
                library.set(lib.books);
            }
        });
    });

    let q = query.read().to_lowercase();
    let books = library.read();
    let filtered: Vec<&EbookMetadata> = books
        .iter()
        .filter(|b| {
            q.is_empty()
                || b.title
                    .as_deref()
                    .unwrap_or(&b.filename)
                    .to_lowercase()
                    .contains(&q)
        })
        .collect();
    let picked_count = picked.read().len();

    rsx! {
        div { class: "shelf-picker",
            div { class: "shelf-picker-bar",
                input {
                    r#type: "search",
                    class: "shelf-picker-search",
                    placeholder: "Search your library\u{2026}",
                    "data-testid": "shelf-picker-search",
                    value: "{query}",
                    oninput: move |e| query.set(e.value()),
                }
                span { class: "mono shelf-picker-count", "On this shelf \u{b7} {picked_count}" }
            }
            div { class: "shelf-picker-grid",
                for book in filtered.iter().map(|b| (*b).clone()) {
                    {picker_tile(book, &server_url, picked, &mut picked)}
                }
            }
        }
    }
}

/// One selectable picker tile; clicking toggles the book's uuid in `picked`.
fn picker_tile(
    book: EbookMetadata,
    server_url: &str,
    picked_read: Signal<Vec<String>>,
    picked: &mut Signal<Vec<String>>,
) -> Element {
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let selected = picked_read.read().contains(&uuid);
    let (src, srcset) = thumb_srcs(&book, &uuid, server_url);
    let mut picked = *picked;
    let toggle_uuid = uuid.clone();
    let class = if selected {
        "shelf-cover-tile shelf-cover-tile--selectable shelf-cover-tile--picked"
    } else {
        "shelf-cover-tile shelf-cover-tile--selectable"
    };

    rsx! {
        button {
            r#type: "button",
            class: "{class}",
            "aria-pressed": if selected { "true" } else { "false" },
            onclick: move |_| {
                let u = toggle_uuid.clone();
                picked.with_mut(|v| {
                    if let Some(pos) = v.iter().position(|x| x == &u) {
                        v.remove(pos);
                    } else {
                        v.push(u);
                    }
                });
            },
            Cover {
                book,
                src_override: src,
                srcset,
                sizes: Some("120px".to_string()),
            }
        }
    }
}
