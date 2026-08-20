//! The compare view a selected candidate opens into: your metadata on the
//! left, the source's on the right, one row per field, and a control on each
//! row that moves that value into yours.
//!
//! Every row comes from iterating [`MetadataField::ALL`] — see
//! [`super::field`] for why that matters. Applying only *stages* into the
//! edit form's own signals; the page's save bar stays the single writer, so
//! dirty tracking and validation keep working untouched.

use dioxus::prelude::*;
use omnibus_shared::metadata_lookup::ProviderEdition;

use super::super::form_grid::FormFields;
use super::field::MetadataField;
use super::sources::provider_slug;

/// Rendered for a field the source has no value for.
const EMPTY: &str = "\u{2014}";

/// The selected edition beside the form's current values, with the control
/// that returns to the results.
#[component]
pub(super) fn ComparePanel(
    edition: ProviderEdition,
    fields: FormFields,
    hydrating: bool,
    on_back: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "mes-compare", "data-testid": "mes-compare",
            div { class: "mes-compare-head",
                button {
                    r#type: "button",
                    class: "btn ghost sm",
                    "data-testid": "mes-compare-back",
                    onclick: move |_| on_back.call(()),
                    "\u{2190} Back to results"
                }
                span {
                    class: "mes-badge",
                    "data-testid": "mes-compare-source",
                    "data-source": "{provider_slug(edition.source)}",
                    "{edition.source.display_name()}"
                }
                if hydrating {
                    span { class: "mono mes-compare-busy", role: "status", "data-testid": "mes-compare-busy",
                        "Fetching the full record\u{2026}"
                    }
                }
            }
            TakeAll { edition: edition.clone(), fields }
            CompareRows { edition: edition.clone(), fields }
        }
    }
}

/// "Take everything from this source" — every field the source answers for,
/// in one action. Fields it has no value for are skipped, not blanked.
#[component]
fn TakeAll(edition: ProviderEdition, fields: FormFields) -> Element {
    let available = MetadataField::ALL
        .iter()
        .filter(|f| f.is_available(&edition))
        .count();
    let take_all = move |_| {
        for field in MetadataField::ALL {
            field.apply(fields, &edition);
        }
    };
    rsx! {
        div { class: "mes-compare-actions",
            button {
                r#type: "button",
                class: "btn primary sm",
                "data-testid": "mes-take-all",
                disabled: available == 0,
                onclick: take_all,
                "Take everything from this source"
            }
            span { class: "mono mes-compare-hint", "data-testid": "mes-staged-hint",
                "staged into the form \u{b7} nothing is saved until you press Save"
            }
        }
    }
}

/// The header row plus one [`CompareRow`] per field.
#[component]
fn CompareRows(edition: ProviderEdition, fields: FormFields) -> Element {
    let source_name = edition.source.display_name();
    rsx! {
        div { class: "mes-compare-grid", "data-testid": "mes-compare-grid",
            div { class: "mes-compare-headrow", aria_hidden: "true",
                span { class: "mes-compare-col", "" }
                span { class: "mes-compare-col", "Yours" }
                span { class: "mes-compare-col", "" }
                span { class: "mes-compare-col", "{source_name}" }
            }
            for field in MetadataField::ALL.iter().copied() {
                CompareRow {
                    key: "{field.slug()}",
                    field,
                    edition: edition.clone(),
                    fields,
                    source_name,
                }
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

    let apply = move |_| field.apply(fields, &edition);

    rsx! {
        div {
            class: if matched { "mes-compare-row mes-compare-row-matched" } else { "mes-compare-row" },
            "data-testid": "mes-row-{slug}",
            "data-applied": if matched { "true" } else { "false" },
            span { class: "mes-compare-label", "{label}" }
            span { class: "mes-compare-current", "data-testid": "mes-row-{slug}-current",
                if current.trim().is_empty() {
                    span { class: "mes-compare-empty", "{EMPTY}" }
                } else {
                    "{current}"
                }
            }
            button {
                r#type: "button",
                class: "mes-compare-apply",
                "data-testid": "mes-row-{slug}-apply",
                // Named rather than inheriting the arrow glyph: ten rows
                // whose accessible name is "→" are ten identical controls.
                aria_label: "Copy {label} from {source_name}",
                // The guard that matters on this screen: a provider not
                // knowing a field must never blank out a value you have.
                disabled: !available,
                onclick: apply,
                "\u{2192}"
            }
            span {
                class: if available { "mes-compare-source" } else { "mes-compare-source mes-compare-empty" },
                "data-testid": "mes-row-{slug}-source",
                if available { "{source_value}" } else { "{EMPTY}" }
            }
        }
    }
}
