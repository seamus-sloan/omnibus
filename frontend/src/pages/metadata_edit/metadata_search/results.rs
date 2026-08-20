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

/// The results screen: query row, then whatever the last search produced.
#[component]
pub(super) fn ResultsScreen(
    state: PickerState,
    stage: Stage,
    on_search: EventHandler<()>,
    on_select: EventHandler<ProviderEdition>,
) -> Element {
    let mut query = state.query;
    let searching = stage == Stage::Searching;

    rsx! {
        div { class: "mes-screen", "data-testid": "mes-results",
            h2 { class: "mes-title", "Find this edition" }
            form {
                class: "mes-query",
                onsubmit: move |e| {
                    e.prevent_default();
                    on_search.call(());
                },
                input {
                    id: "mes-query",
                    class: "mes-query-input",
                    "data-testid": "mes-query",
                    r#type: "search",
                    value: "{query}",
                    aria_label: "Search providers",
                    placeholder: "title and author",
                    onmounted: move |e| focus_after_paint(&e),
                    oninput: move |e| query.set(e.value()),
                }
                button {
                    r#type: "submit",
                    class: "btn mes-query-btn",
                    "data-testid": "mes-search",
                    disabled: searching || query().trim().is_empty(),
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
