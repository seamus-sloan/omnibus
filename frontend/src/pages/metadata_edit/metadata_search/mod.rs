//! The metadata editor's edition picker: search every configured provider at
//! once, list what each one found, and hand one candidate off to the compare
//! view.
//!
//! Rule 07 shapes the top of this module: the availability check and every
//! fetch live in post-mount effects and handlers, never in a `cfg` around the
//! rsx, so SSR and the first WASM paint agree on an empty panel and the
//! reveal happens once — after hydration has already adopted the DOM.

use dioxus::prelude::*;
use omnibus_shared::metadata_lookup::{EditionSearchResponse, ProviderEdition};

use super::form_grid::FormFields;
use crate::{data, use_server_url};

mod candidates;
mod compare;
mod field;
mod sources;

use candidates::CandidateList;
use compare::ComparePanel;
use sources::SourceStatusList;

/// What the search is currently showing.
#[derive(Clone, PartialEq)]
enum SearchPhase {
    /// Opened but never run.
    Idle,
    Searching,
    Results(EditionSearchResponse),
    /// The request itself failed — distinct from a search that ran and found
    /// nothing, which is a `Results` with an empty list and a status row per
    /// source explaining why.
    Error(String),
}

/// The picker's signals, bundled so the handlers don't each thread six
/// separate params (mirrors `cover_editor::CoverState`).
#[derive(Clone, Copy, PartialEq)]
struct PickerState {
    open: Signal<bool>,
    query: Signal<String>,
    phase: Signal<SearchPhase>,
    selected: Signal<Option<ProviderEdition>>,
    hydrating: Signal<bool>,
}

/// Slugify a field label for a `data-testid`, matching `fields::label_to_id`'s
/// rule without its `me-` prefix.
fn field_slug(label: &str) -> String {
    label
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect()
}

/// The search query a freshly-opened panel starts from: the book's current
/// title and primary author, which is the query the reader wanted in the
/// overwhelmingly common case.
///
/// Read with `peek` rather than a call: seeding must not subscribe the panel
/// to the title field, or typing in the form would re-render it on every
/// keystroke.
fn seed_query(fields: FormFields) -> String {
    let title = fields.title.peek().clone();
    let author = fields.authors.peek().first().cloned();
    compose_query(&title, author.as_deref())
}

/// The seeded query itself, split out from the signal reads so the rule it
/// encodes — never emit a leading or trailing space, and never a query that
/// is only whitespace — is testable without a Dioxus runtime.
fn compose_query(title: &str, author: Option<&str>) -> String {
    let title = title.trim();
    let author = author.map(str::trim).unwrap_or_default();
    match (title.is_empty(), author.is_empty()) {
        (true, _) => author.to_string(),
        (false, true) => title.to_string(),
        (false, false) => format!("{title} {author}"),
    }
}

/// Top-level picker panel, mounted in the edit form's left column.
///
/// Renders nothing at all until the provider catalog says at least one
/// source is usable — offering a search that could only fail is worse than
/// not offering one.
#[component]
pub(super) fn MetadataSearchPanel(fields: FormFields) -> Element {
    let server_url = use_server_url();
    let mut available = use_signal(|| false);
    // Declared ahead of the availability gate, unconditionally: a hook count
    // that changes across the reveal panics Dioxus (rule 07).
    let state = PickerState {
        open: use_signal(|| false),
        query: use_signal(|| seed_query(fields)),
        phase: use_signal(|| SearchPhase::Idle),
        selected: use_signal(|| None),
        hydrating: use_signal(|| false),
    };

    let url = server_url.clone();
    use_effect(move || {
        let url = url.clone();
        spawn(async move {
            if let Ok(providers) = data::list_metadata_providers(&url).await {
                available.set(providers.iter().any(|p| p.configured));
            }
        });
    });

    if !available() {
        return rsx! {};
    }

    rsx! {
        div { class: "mes-panel", "data-testid": "metadata-search",
            OpenButton { state }
            if (state.open)() {
                PickerBody { state, fields, server_url }
            }
        }
    }
}

/// The entry point: opens the panel, and closes it again without discarding
/// what the reader had searched for.
#[component]
fn OpenButton(state: PickerState) -> Element {
    let mut open = state.open;
    rsx! {
        button {
            r#type: "button",
            class: "btn mes-open-btn",
            "data-testid": "metadata-search-btn",
            aria_expanded: if open() { "true" } else { "false" },
            onclick: move |_| open.toggle(),
            if open() { "Hide edition search" } else { "Search for this edition" }
        }
    }
}

/// The opened panel: the query row, then either the compare view for a
/// selected candidate or the results for the last search.
#[component]
fn PickerBody(state: PickerState, fields: FormFields, server_url: String) -> Element {
    let on_search = search_handler(state, server_url.clone());
    let on_select = select_handler(state, server_url);
    let mut selected = state.selected;

    rsx! {
        div { class: "mes-body",
            QueryRow { state, on_search }
            if let Some(edition) = selected() {
                ComparePanel {
                    edition,
                    fields,
                    hydrating: (state.hydrating)(),
                    on_back: move |()| selected.set(None),
                }
            } else {
                Results { state, on_select }
            }
        }
    }
}

/// The editable query and the button that runs it. Submitting the form (the
/// Enter key) runs the same search the button does — a search box that
/// ignores Enter is the kind of thing only the person who wrote it doesn't
/// notice.
#[component]
fn QueryRow(state: PickerState, on_search: EventHandler<()>) -> Element {
    let mut query = state.query;
    let busy = (state.phase)() == SearchPhase::Searching;
    rsx! {
        form {
            class: "mes-query",
            onsubmit: move |e| {
                e.prevent_default();
                on_search.call(());
            },
            label { class: "me-label", r#for: "mes-query-input", span { "Search providers" } }
            div { class: "mes-query-row",
                input {
                    id: "mes-query-input",
                    class: "me-input",
                    "data-testid": "mes-query",
                    value: "{query}",
                    placeholder: "title and author",
                    oninput: move |e| query.set(e.value()),
                }
                button {
                    r#type: "submit",
                    class: "btn primary",
                    "data-testid": "mes-search",
                    disabled: busy || query().trim().is_empty(),
                    if busy { "Searching\u{2026}" } else { "Search" }
                }
            }
        }
    }
}

/// Whatever the last search produced: nothing yet, a spinner, the candidates
/// plus their per-source status, or the request's own failure.
#[component]
fn Results(state: PickerState, on_select: EventHandler<ProviderEdition>) -> Element {
    rsx! {
        {match (state.phase)() {
            SearchPhase::Idle => rsx! {},
            SearchPhase::Searching => rsx! {
                p { class: "mes-note", role: "status", "data-testid": "mes-searching",
                    "Asking every configured source\u{2026}"
                }
            },
            SearchPhase::Error(msg) => rsx! {
                p { class: "mes-error", role: "alert", "data-testid": "mes-error", "{msg}" }
            },
            SearchPhase::Results(found) => rsx! {
                CandidateList { editions: found.editions, on_select }
                SourceStatusList { sources: found.sources }
            },
        }}
    }
}

/// Run the fan-out search for the current query.
fn search_handler(state: PickerState, server_url: String) -> EventHandler<()> {
    let mut phase = state.phase;
    let query = state.query;
    let mut selected = state.selected;
    EventHandler::new(move |()| {
        let q = query.peek().trim().to_string();
        if q.is_empty() || *phase.peek() == SearchPhase::Searching {
            return;
        }
        let url = server_url.clone();
        // A new search invalidates the edition the last one was comparing.
        selected.set(None);
        phase.set(SearchPhase::Searching);
        spawn(async move {
            match data::search_editions(&url, &q).await {
                Ok(found) => phase.set(SearchPhase::Results(found)),
                Err(e) => phase.set(SearchPhase::Error(e.to_string())),
            }
        });
    })
}

/// Open the compare view for one candidate, and re-fetch it in full behind
/// the reveal.
///
/// The merge is one-directional on purpose: the detail record fills in what
/// the list row lacked and can never take a field away from it, so the row
/// the reader clicked is still the row they get.
fn select_handler(state: PickerState, server_url: String) -> EventHandler<ProviderEdition> {
    let mut selected = state.selected;
    let mut hydrating = state.hydrating;
    EventHandler::new(move |edition: ProviderEdition| {
        selected.set(Some(edition.clone()));
        hydrating.set(true);
        let url = server_url.clone();
        spawn(async move {
            let fetched =
                data::hydrate_edition(&url, edition.source, &edition.provider_ref, &edition.isbn13)
                    .await;
            // A slow hydrate must not overwrite a candidate the reader has
            // since replaced — or reappear after they went back to the list.
            let still_showing = is_still_selected(&selected, &edition);
            if still_showing {
                if let Ok(Some(mut full)) = fetched {
                    full.fill_missing_from(&edition);
                    selected.set(Some(full));
                }
            }
            // Left alone only when a *newer* selection is in flight — that
            // request owns the flag and will clear it when it lands.
            if still_showing || selected.peek().is_none() {
                hydrating.set(false);
            }
        });
    })
}

/// Whether `edition` is still the candidate the compare view is showing.
/// Identity is `(source, provider_ref)` — the ISBN alone is not enough, since
/// the picker deliberately keeps two sources' rows for one ISBN apart.
fn is_still_selected(
    selected: &Signal<Option<ProviderEdition>>,
    edition: &ProviderEdition,
) -> bool {
    selected
        .peek()
        .as_ref()
        .is_some_and(|s| s.source == edition.source && s.provider_ref == edition.provider_ref)
}

#[cfg(test)]
mod tests;
