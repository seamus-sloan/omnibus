//! Screen one: what the providers found.
//!
//! The query sits at the top, already filled in and already run, so the
//! reader arrives at answers rather than at a form. Everything else on the
//! screen is one row per candidate — cover, title, authors, one line of
//! imprint, and which source offered it.

use dioxus::prelude::*;
use omnibus_shared::metadata_lookup::ProviderEdition;

use super::candidates::CandidateRow;
use super::sources::SourceSummary;
use super::{PickerState, Stage};
use crate::focus_after_paint::focus_after_paint;

/// One labelled field of the query.
///
/// Separate fields rather than one box because that is the only way to say
/// which part is which — and each provider is asked in its own terms, so the
/// distinction is load-bearing rather than cosmetic. Open Library matches
/// `title=` against the title field alone: hand it "Dune Frank Herbert" and it
/// answers with books written *about* Dune.
#[component]
fn QueryField(
    slug: &'static str,
    label: &'static str,
    value: Signal<String>,
    autofocus: bool,
) -> Element {
    let mut value = value;
    rsx! {
        label { class: "mes-query-field", r#for: "mes-query-{slug}",
            span { class: "mes-query-label", "{label}" }
            input {
                id: "mes-query-{slug}",
                class: "mes-query-input",
                "data-testid": "mes-query-{slug}",
                r#type: if slug == "isbn" { "text" } else { "search" },
                value: "{value}",
                placeholder: "{label.to_lowercase()}",
                // The overlay opens having already searched, so focus lands on
                // the field a reader is most likely to correct.
                onmounted: move |e| {
                    if autofocus {
                        focus_after_paint(&e);
                    }
                },
                oninput: move |e| value.set(e.value()),
            }
        }
    }
}

/// The results screen: query fields, then whatever the last search produced.
#[component]
pub(super) fn ResultsScreen(
    state: PickerState,
    stage: Stage,
    on_search: EventHandler<()>,
    on_select: EventHandler<ProviderEdition>,
) -> Element {
    let searching = stage == Stage::Searching;
    let nothing_to_ask = super::request_from(state).is_none();

    rsx! {
        div { class: "mes-screen mes-screen-results", "data-testid": "mes-results",
            h2 { class: "mes-title", "Find this edition" }
            form {
                class: "mes-query",
                onsubmit: move |e| {
                    e.prevent_default();
                    on_search.call(());
                },
                QueryField {
                    slug: "title",
                    label: "Title",
                    value: state.title,
                    autofocus: true,
                }
                QueryField {
                    slug: "author",
                    label: "Author",
                    value: state.author,
                    autofocus: false,
                }
                QueryField {
                    slug: "isbn",
                    label: "ISBN",
                    value: state.isbn,
                    autofocus: false,
                }
                button {
                    r#type: "submit",
                    class: "btn mes-query-btn",
                    "data-testid": "mes-search",
                    // Any one field is enough: an ISBN alone is the strongest
                    // question there is, and a title alone is the commonest.
                    disabled: searching || nothing_to_ask,
                    if searching { "Searching\u{2026}" } else { "Search" }
                }
            }
            {match stage {
                Stage::Searching => rsx! {
                    p { class: "mes-note", role: "status", "data-testid": "mes-searching",
                        "Asking every configured source\u{2026}"
                    }
                },
                Stage::Failed(msg) => rsx! {
                    p { class: "mes-error", role: "alert", "data-testid": "mes-error", "{msg}" }
                },
                _ => rsx! {
                    CandidateList { state, on_select }
                },
            }}
        }
    }
}

/// The candidates, or the note that stands in for an empty list.
#[component]
fn CandidateList(state: PickerState, on_select: EventHandler<ProviderEdition>) -> Element {
    let editions = (state.editions)();
    rsx! {
        if editions.is_empty() {
            p { class: "mes-note", role: "status", "data-testid": "mes-empty",
                "No editions matched. Try a shorter title, or just the author."
            }
        } else {
            ul { class: "mes-list", "data-testid": "mes-candidates",
                for (index , edition) in editions.into_iter().enumerate() {
                    CandidateRow { key: "{index}", index, edition, on_select }
                }
            }
        }
        SourceSummary { sources: (state.sources)() }
    }
}
