//! Title search — the fallback offered when an ISBN resolves to nothing. A
//! query input over a provider-candidate list; picking a candidate hands it
//! back to the flow, which resolves it against the library without a second
//! provider round trip. Visible text is the E2E selector contract (rule 04).

use dioxus::prelude::*;
use omnibus_shared::{ExternalBookMeta, ScanSearchRequest};

use super::screens::ExternalBookCard;
use super::{friendly_error, FlowState};
use crate::{data, use_server_url};

/// Title search over the metadata providers. The search read is owned here
/// (like `ScanScreen`'s configured-key fetch); the pick goes back out through
/// `on_pick` so the flow owns the stage transition.
#[component]
pub(super) fn SearchScreen(
    state: FlowState,
    on_pick: EventHandler<ExternalBookMeta>,
    on_restart: EventHandler<()>,
) -> Element {
    let server_url = use_server_url();
    let busy = state.busy;
    let mut error = state.error;
    let mut query = use_signal(String::new);
    let mut searching = use_signal(|| false);
    // `None` until the first search lands, so the empty-state line can't show
    // before anyone has searched.
    let mut results = use_signal(|| None::<Vec<ExternalBookMeta>>);

    let mut run_search = move |_| {
        let trimmed = query().trim().to_string();
        if trimmed.is_empty() || searching() {
            return;
        }
        let server_url = server_url.clone();
        error.set(None);
        searching.set(true);
        spawn(async move {
            let req = ScanSearchRequest { query: trimmed };
            match data::scan_search(&server_url, req).await {
                Ok(found) => results.set(Some(found.results)),
                Err(e) => error.set(Some(format!(
                    "Could not search: {}",
                    friendly_error(&e.to_string())
                ))),
            }
            searching.set(false);
        });
    };

    rsx! {
        div { class: "check-in-screen", "data-testid": "check-in-search",
            h1 { "Search by title" }
            p { class: "subtitle",
                "Type the book's name and we'll look it up online."
            }
            form {
                class: "settings-form",
                "data-testid": "check-in-search-form",
                onsubmit: move |evt| {
                    evt.prevent_default();
                    run_search(());
                },
                div { class: "settings-field",
                    label { r#for: "check-in-search-query", "Title" }
                    input {
                        id: "check-in-search-query",
                        "data-testid": "check-in-search-query",
                        r#type: "search",
                        autocomplete: "off",
                        placeholder: "The Name of the Wind",
                        value: "{query}",
                        disabled: busy() || searching(),
                        oninput: move |e| query.set(e.value()),
                    }
                }
                div { class: "settings-actions",
                    button {
                        r#type: "submit",
                        class: "btn primary",
                        disabled: busy() || searching() || query().trim().is_empty(),
                        "data-testid": "check-in-search-submit",
                        if searching() { "Searching\u{2026}" } else { "Search" }
                    }
                    button {
                        r#type: "button",
                        class: "btn ghost",
                        disabled: busy() || searching(),
                        "data-testid": "check-in-search-restart",
                        onclick: move |_| on_restart.call(()),
                        "Scan a barcode instead"
                    }
                }
            }
            match results() {
                Some(list) if list.is_empty() => rsx! {
                    p { class: "check-in-why", "data-testid": "check-in-search-empty",
                        "No books match that title. Check the spelling, or try just the main title."
                    }
                },
                Some(list) => rsx! {
                    ul { class: "check-in-search-results", "data-testid": "check-in-search-results",
                        for meta in list {
                            li { key: "{meta.isbn13}",
                                SearchResult { meta, busy, on_pick }
                            }
                        }
                    }
                },
                None => rsx! {},
            }
        }
    }
}

/// One pickable candidate row — the shared provider card inside a button.
#[component]
fn SearchResult(
    meta: ExternalBookMeta,
    busy: Signal<bool>,
    on_pick: EventHandler<ExternalBookMeta>,
) -> Element {
    let picked = meta.clone();
    rsx! {
        button {
            r#type: "button",
            class: "check-in-search-result",
            "data-testid": "check-in-search-result",
            disabled: busy(),
            onclick: move |_| on_pick.call(picked.clone()),
            ExternalBookCard { meta }
        }
    }
}
