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

/// Above this many characters on either side, a row stops being readable in
/// two narrow columns and is laid out down the panel's full width instead.
/// Sized to roughly three lines of a compare column at desktop width.
const LONG_VALUE_CHARS: usize = 90;

/// The selected edition beside the book, and the controls that move fields
/// across.
#[component]
pub(super) fn CompareScreen(
    edition: ProviderEdition,
    fields: FormFields,
    orig: Signal<EbookMetadata>,
    uuid: String,
    book: EbookMetadata,
    hydrating: bool,
    on_back: EventHandler<()>,
    on_cover_applied: EventHandler<EbookMetadata>,
    on_done: EventHandler<()>,
) -> Element {
    let mut show_all = use_signal(|| false);
    let source_name = edition.source.display_name();
    let baseline = orig();
    // Staged is measured against the book's frozen baseline, not against what
    // this screen happened to do — so a row stays marked after a trip back to
    // the results, a field edited directly in the form is marked too, and the
    // count here can never disagree with the save bar's.
    let staged: Vec<MetadataField> = MetadataField::ALL
        .iter()
        .copied()
        .filter(|f| f.is_staged(fields, &baseline))
        .collect();
    let has_source_cover = edition
        .cover_url
        .as_deref()
        .is_some_and(|u| !u.trim().is_empty());

    // A staged row stays on screen even once it stops differing: applying a
    // field makes the two sides agree, and a row that vanished the instant
    // you pressed it would take the evidence of what you did with it.
    let shown: Vec<MetadataField> = MetadataField::ALL
        .iter()
        .copied()
        .filter(|f| show_all() || f.differs(fields, &edition) || staged.contains(f))
        .collect();
    let changes = MetadataField::ALL
        .iter()
        .filter(|f| f.differs(fields, &edition))
        .count();

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
            // Nothing real is shown until the record has settled. The search
            // hit this screen opens with is a partial answer — it carries
            // fewer fields than the provider's own record — so rendering it
            // means rows appearing and rearranging under the reader a moment
            // later. A placeholder for that moment is calmer than a wrong
            // answer corrected in view.
            if hydrating {
                p { class: "mes-subtitle", role: "status", "data-testid": "mes-hydrating",
                    "Loading the full record\u{2026}"
                }
                CompareSkeleton {}
            } else {
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
                            orig,
                            source_name,
                            hydrating,
                        }
                    }
                }
            }

            div { class: "mes-footbar",
                button {
                    r#type: "button",
                    class: "btn primary",
                    "data-testid": "mes-take-all",
                    disabled: hydrating || changes == 0,
                    onclick: move |_| {
                        for field in MetadataField::ALL {
                            field.apply(fields, &edition);
                        }
                    },
                    "Take all"
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
                p { class: "mono mes-foot", "data-testid": "mes-staged-hint",
                    "Fields aren't saved until you press Save on the form."
                }
            }
        }
    }
}

/// What the compare screen shows while the record is still arriving: the
/// shape of the answer, without any of its content.
///
/// Deliberately not a spinner. The reader is about to read a table, and a
/// placeholder in that table's shape means the real rows land where the eye
/// is already looking instead of shifting it.
#[component]
fn CompareSkeleton() -> Element {
    rsx! {
        div { class: "mes-skeleton", "data-testid": "mes-compare-skeleton", aria_hidden: "true",
            div { class: "mes-skel-cover" }
            for i in 0..5 {
                div { key: "{i}", class: "mes-skel-row",
                    span { class: "mes-skel-bar mes-skel-label" }
                    span { class: "mes-skel-bar" }
                    span { class: "mes-skel-bar" }
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
    orig: Signal<EbookMetadata>,
    source_name: &'static str,
    hydrating: bool,
) -> Element {
    let label = field.label();
    let slug = field.slug();
    let baseline = orig();
    let source_value = field.source_value(&edition);
    let available = !source_value.trim().is_empty();
    let current = field.current(fields);
    let staged = field.is_staged(fields, &baseline);
    // Driven by the value, not the field: a description is the usual reason a
    // row needs the room, but a long title or a long publisher imprint is the
    // same problem, and neither is worth its own case.
    let long = current.chars().count() > LONG_VALUE_CHARS
        || source_value.chars().count() > LONG_VALUE_CHARS;

    rsx! {
        div {
            class: if long { "mes-field mes-field-long" } else { "mes-field" },
            "data-testid": "mes-row-{slug}",
            "data-staged": if staged { "true" } else { "false" },
            span { class: "mes-field-label", "{label}" }
            span { class: "mes-field-mine", "data-testid": "mes-row-{slug}-current",
                if current.trim().is_empty() {
                    span { class: "mes-empty", "{EMPTY}" }
                } else {
                    "{current}"
                }
            }
            // One control, two meanings, so a row's state is readable from it
            // alone: an arrow to take the source's value, and — once the field
            // carries a change — the way back to what the book had.
            if staged {
                button {
                    r#type: "button",
                    class: "mes-apply mes-undo",
                    "data-testid": "mes-row-{slug}-undo",
                    aria_label: "Undo the change to {label}",
                    disabled: hydrating,
                    onclick: move |_| field.undo(fields, &orig()),
                    "\u{21a9}"
                }
            } else {
                button {
                    r#type: "button",
                    class: "mes-apply",
                    "data-testid": "mes-row-{slug}-apply",
                    // Named rather than inheriting the arrow glyph: a screenful
                    // of controls all called "→" is a screenful of identical
                    // controls.
                    aria_label: "Copy {label} from {source_name}",
                    // Two guards. A provider not knowing a field must never
                    // blank out a value you have — and neither must a row about
                    // to be replaced: while the detail re-fetch is in flight
                    // this record is the thin search hit, not the answer.
                    disabled: !available || hydrating,
                    onclick: move |_| field.apply(fields, &edition),
                    "\u{2192}"
                }
            }
            span {
                class: if available { "mes-field-theirs" } else { "mes-field-theirs mes-empty" },
                "data-testid": "mes-row-{slug}-source",
                if available { "{source_value}" } else { "{EMPTY}" }
            }
        }
    }
}
