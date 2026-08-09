//! Link a scanned copy to a book the library already holds.
//!
//! The escape hatch for the copies no rung of the matching ladder can bridge:
//! a print edition whose barcode the library book doesn't publish, under a
//! provider title that shares nothing with it (pre-release placeholders are
//! the usual culprit). The reader names the book instead of the matcher
//! guessing it. Visible text is the E2E selector contract (rule 04).

use dioxus::prelude::*;
use omnibus_shared::{PaletteBookHit, ScanBook};

use super::screens::LibraryBookCard;
use super::{friendly_error, FlowState};
use crate::{data, use_server_url};

/// Split the palette's pre-joined `author_display` back into names, matching
/// how `db::scan` splits its own `group_concat`ed author list.
fn split_authors(display: &str) -> Vec<String> {
    display
        .split(", ")
        .map(str::trim)
        .filter(|a| !a.is_empty())
        .map(str::to_string)
        .collect()
}

/// Turn a library-search hit into the [`ScanBook`] the confirm screen renders.
///
/// `has_physical` stays `false` because the search projection carries no copy
/// count — and nothing on this path reads it. The confirm card shows cover,
/// title, and byline only, and the write resolves ownership server-side.
pub(crate) fn scan_book_from_hit(hit: &PaletteBookHit) -> ScanBook {
    ScanBook {
        uuid: hit.uuid.clone(),
        title: hit.title.clone(),
        authors: split_authors(&hit.author_display),
        cover_url: hit.cover_url.clone(),
        has_physical: false,
        isbn: None,
    }
}

/// The line under a truncated result list. The palette caps each category at
/// five hits, so a broad query hides matches; say so rather than letting the
/// reader conclude their book isn't there. `None` when nothing is hidden.
pub(crate) fn truncation_note(shown: usize, total: u32) -> Option<String> {
    (total as usize > shown).then(|| {
        format!("Showing {shown} of {total} matches \u{2014} add the author or more of the title.")
    })
}

/// Search the library and pick the book this copy belongs to. The pick goes
/// back out through `on_pick` so the flow owns the stage transition and the
/// write, exactly like the provider title search does.
#[component]
pub(super) fn LinkExistingScreen(
    state: FlowState,
    on_pick: EventHandler<ScanBook>,
    on_back: EventHandler<()>,
) -> Element {
    let server_url = use_server_url();
    let busy = state.busy;
    let query = use_signal(String::new);
    let searching = use_signal(|| false);
    // `None` until the first search lands, so the empty-state line can't show
    // before anyone has searched.
    let results = use_signal(|| None::<(Vec<PaletteBookHit>, u32)>);
    let run_search = build_library_search_handler(server_url, query, state, searching, results);

    rsx! {
        div { class: "check-in-screen", "data-testid": "check-in-link",
            h1 { "Which book is this?" }
            p { class: "subtitle",
                "Find it in your library and we'll file this copy against it \u{2014} no new book, no duplicate to merge later."
            }
            LibrarySearchForm {
                query,
                busy,
                searching,
                on_submit: EventHandler::new(run_search),
            }
            LibrarySearchResults { results: results(), busy, on_pick }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn ghost",
                    disabled: busy(),
                    "data-testid": "check-in-link-back",
                    onclick: move |_| on_back.call(()),
                    "Back"
                }
            }
        }
    }
}

/// Builds the library-search submit handler: runs `data::search_palette` for
/// the trimmed query and populates `results` (or the flow's `error`).
fn build_library_search_handler(
    server_url: String,
    query: Signal<String>,
    state: FlowState,
    mut searching: Signal<bool>,
    mut results: Signal<Option<(Vec<PaletteBookHit>, u32)>>,
) -> impl FnMut(()) + 'static {
    let mut error = state.error;
    move |_: ()| {
        let trimmed = query().trim().to_string();
        if trimmed.is_empty() || searching() {
            return;
        }
        let server_url = server_url.clone();
        error.set(None);
        searching.set(true);
        spawn(async move {
            match data::search_palette(&server_url, &trimmed).await {
                Ok(found) => results.set(Some((found.books, found.book_total))),
                Err(e) => error.set(Some(format!(
                    "Could not search your library: {}",
                    friendly_error(&e.to_string())
                ))),
            }
            searching.set(false);
        });
    }
}

/// The query input + submit button.
#[component]
fn LibrarySearchForm(
    mut query: Signal<String>,
    busy: Signal<bool>,
    searching: Signal<bool>,
    on_submit: EventHandler<()>,
) -> Element {
    rsx! {
        form {
            class: "settings-form",
            "data-testid": "check-in-link-form",
            onsubmit: move |evt| {
                evt.prevent_default();
                on_submit.call(());
            },
            div { class: "settings-field",
                label { r#for: "check-in-link-query", "Title or author" }
                input {
                    id: "check-in-link-query",
                    "data-testid": "check-in-link-query",
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
                    "data-testid": "check-in-link-submit",
                    if searching() { "Searching\u{2026}" } else { "Search my library" }
                }
            }
        }
    }
}

/// Empty-state note, the pickable list, or nothing before a first search.
#[component]
fn LibrarySearchResults(
    results: Option<(Vec<PaletteBookHit>, u32)>,
    busy: Signal<bool>,
    on_pick: EventHandler<ScanBook>,
) -> Element {
    match results {
        Some((list, _)) if list.is_empty() => rsx! {
            p { class: "check-in-why", "data-testid": "check-in-link-empty",
                "No book in your library matches that. Try the author's name, or a distinctive word from the title."
            }
        },
        Some((list, total)) => {
            let note = truncation_note(list.len(), total);
            rsx! {
                ul { class: "check-in-search-results", "data-testid": "check-in-link-results",
                    for hit in list {
                        li { key: "{hit.uuid}",
                            LibraryResult { hit, busy, on_pick }
                        }
                    }
                }
                if let Some(note) = note {
                    p { class: "check-in-why", "data-testid": "check-in-link-truncated", "{note}" }
                }
            }
        }
        None => rsx! {},
    }
}

/// One pickable library book — the shared library card inside a button.
#[component]
fn LibraryResult(
    hit: PaletteBookHit,
    busy: Signal<bool>,
    on_pick: EventHandler<ScanBook>,
) -> Element {
    let book = scan_book_from_hit(&hit);
    let picked = book.clone();
    rsx! {
        button {
            r#type: "button",
            class: "check-in-search-result",
            "data-testid": "check-in-link-result",
            disabled: busy(),
            onclick: move |_| on_pick.call(picked.clone()),
            LibraryBookCard { book }
        }
    }
}
