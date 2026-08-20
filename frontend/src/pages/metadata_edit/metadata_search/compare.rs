//! Screen two: what this source would change.
//!
//! Only the fields where the source disagrees with your book, because the
//! question the screen answers is "what would this do?" — and on a typical
//! book that is two or three rows, not the eleven a full record has. The rest
//! is one toggle away for the times you want to see what a source says about
//! a field you have already filled in.
//!
//! Applying **stages** into the edit form's own signals: the page's save bar
//! stays the single writer, so dirty tracking and validation keep working
//! untouched. The **cover is the one exception** — it cannot stage, so it
//! writes immediately and says so; see [`super::cover_row`].

use dioxus::prelude::*;
use omnibus_shared::{metadata_lookup::ProviderEdition, EbookMetadata};

use super::super::form_grid::FormFields;
use super::cover_row::CoverRow;
use super::field::MetadataField;
use super::sources::provider_slug;

/// Rendered for a field the source has no value for.
const EMPTY: &str = "\u{2014}";

/// The selected edition beside the book, and the controls that move fields
/// across.
#[component]
pub(super) fn CompareScreen(
    edition: ProviderEdition,
    fields: FormFields,
    uuid: String,
    book: EbookMetadata,
    hydrating: bool,
    on_back: EventHandler<()>,
    on_cover_applied: EventHandler<EbookMetadata>,
    on_done: EventHandler<()>,
) -> Element {
    let mut show_all = use_signal(|| false);
    // Fields taken during this visit. Without it a row vanishes the instant
    // you press its arrow — applying makes it stop differing — so the list
    // shuffles under the cursor and you lose sight of what you just did.
    // They stay, marked as taken, until the screen is left.
    let mut taken: Signal<Vec<MetadataField>> = use_signal(Vec::new);
    let source_name = edition.source.display_name();
    let has_source_cover = edition
        .cover_url
        .as_deref()
        .is_some_and(|u| !u.trim().is_empty());

    let shown: Vec<MetadataField> = MetadataField::ALL
        .iter()
        .copied()
        .filter(|f| show_all() || f.differs(fields, &edition) || taken.read().contains(f))
        .collect();
    let changes = MetadataField::ALL
        .iter()
        .filter(|f| f.differs(fields, &edition))
        .count();
    let take_all_label = if changes == 1 {
        "Take the change".to_string()
    } else {
        format!("Take all {changes}")
    };

    rsx! {
        div { class: "mes-screen", "data-testid": "mes-compare",
            button {
                r#type: "button",
                class: "mes-back",
                "data-testid": "mes-compare-back",
                onclick: move |_| on_back.call(()),
                "\u{2190} All results"
            }
            div { class: "mes-compare-head",
                h2 { class: "mes-title", "{edition.title}" }
                span {
                    class: "mes-badge",
                    "data-testid": "mes-compare-source",
                    "data-source": "{provider_slug(edition.source)}",
                    "{source_name}"
                }
            }
            p { class: "mes-subtitle", "data-testid": "mes-change-count",
                if hydrating {
                    "Loading the full record\u{2026}"
                } else if changes == 0 {
                    "Nothing here differs from your book."
                } else if changes == 1 {
                    "1 field differs from your book."
                } else {
                    "{changes} fields differ from your book."
                }
            }

            // Held to the same rule as every other row: a source with no
            // cover has nothing to offer here, so the row is only noise.
            if has_source_cover || show_all() {
                CoverRow {
                    uuid,
                    book,
                    edition: edition.clone(),
                    source_name,
                    hydrating,
                    on_applied: on_cover_applied,
                }
            }

            div { class: "mes-fields", "data-testid": "mes-compare-fields",
                for field in shown {
                    CompareRow {
                        key: "{field.slug()}",
                        field,
                        edition: edition.clone(),
                        fields,
                        source_name,
                        hydrating,
                        on_take: move |f: MetadataField| taken.write().push(f),
                    }
                }
            }

            div { class: "mes-actions",
                button {
                    r#type: "button",
                    class: "btn primary",
                    "data-testid": "mes-take-all",
                    disabled: hydrating || changes == 0,
                    onclick: move |_| {
                        for field in MetadataField::ALL {
                            if field.differs(fields, &edition) {
                                field.apply(fields, &edition);
                                taken.write().push(*field);
                            }
                        }
                    },
                    "{take_all_label}"
                }
                button {
                    r#type: "button",
                    class: "btn ghost",
                    "data-testid": "mes-show-all",
                    aria_pressed: if show_all() { "true" } else { "false" },
                    onclick: move |_| show_all.toggle(),
                    if show_all() { "Only differences" } else { "Show all fields" }
                }
                button {
                    r#type: "button",
                    class: "btn ghost mes-done",
                    "data-testid": "mes-done",
                    onclick: move |_| on_done.call(()),
                    "Done"
                }
            }
            p { class: "mono mes-foot", "data-testid": "mes-staged-hint",
                "Changes are staged \u{2014} press Save on the form to keep them."
            }
        }
    }
}

/// One field: your value, the arrow that copies the source's into it, and the
/// source's value.
///
/// The control is a button rather than a checkbox on purpose. The ask is
/// "move that field into mine", which is an action with immediate feedback —
/// a checkbox defers it to a second click and leaves the row's state
/// ambiguous once the value has already been copied.
#[component]
fn CompareRow(
    field: MetadataField,
    edition: ProviderEdition,
    fields: FormFields,
    source_name: &'static str,
    hydrating: bool,
    on_take: EventHandler<MetadataField>,
) -> Element {
    let label = field.label();
    let slug = field.slug();
    let source_value = field.source_value(&edition);
    let available = !source_value.trim().is_empty();
    let current = field.current(fields);
    // Not a disabled state: re-applying is harmless, and greying out the
    // control the moment it worked is how a reader loses track of whether
    // they pressed it.
    let matched = available && current == source_value;

    rsx! {
        div {
            class: if matched { "mes-field mes-field-matched" } else { "mes-field" },
            "data-testid": "mes-row-{slug}",
            "data-applied": if matched { "true" } else { "false" },
            span { class: "mes-field-label", "{label}" }
            span { class: "mes-field-mine", "data-testid": "mes-row-{slug}-current",
                if current.trim().is_empty() {
                    span { class: "mes-empty", "{EMPTY}" }
                } else {
                    "{current}"
                }
            }
            button {
                r#type: "button",
                class: "mes-apply",
                "data-testid": "mes-row-{slug}-apply",
                // Named rather than inheriting the arrow glyph: a screenful of
                // controls all called "→" is a screenful of identical controls.
                aria_label: "Copy {label} from {source_name}",
                // Two guards. A provider not knowing a field must never blank
                // out a value you have — and neither must a row that is about
                // to be replaced: while the detail re-fetch is in flight this
                // record is the thin search hit, not the answer.
                disabled: !available || hydrating,
                onclick: move |_| {
                    field.apply(fields, &edition);
                    on_take.call(field);
                },
                "\u{2192}"
            }
            span {
                class: if available { "mes-field-theirs" } else { "mes-field-theirs mes-empty" },
                "data-testid": "mes-row-{slug}-source",
                if available { "{source_value}" } else { "{EMPTY}" }
            }
        }
    }
}
