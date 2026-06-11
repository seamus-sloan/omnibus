//! Admin-only "Merge with…" dialog mounted by the book detail page.
//!
//! Search-and-confirm flow for the F5.10 manual format merge: the admin
//! picks another book via FTS search, confirms, and `data::merge_books`
//! absorbs it into the page's book (the current book is always the
//! merge **target** — its metadata wins; to merge the other way, open
//! the other book's page). The parent receives the
//! [`MergeBooksResult`] for its refetch + undo toast.

use dioxus::core::Task;
use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, MergeBooksResult};

use crate::{data, use_server_url};

/// Modal dialog: search input → candidate rows → confirm panel.
#[component]
pub fn MergeDialog(
    target_uuid: String,
    target_title: String,
    on_merged: EventHandler<MergeBooksResult>,
    on_close: EventHandler<()>,
) -> Element {
    let server_url = use_server_url();
    let mut query = use_signal(String::new);
    let mut results: Signal<Vec<EbookMetadata>> = use_signal(Vec::new);
    let mut selected: Signal<Option<EbookMetadata>> = use_signal(|| None);
    let mut error: Signal<Option<String>> = use_signal(|| None);
    let mut busy = use_signal(|| false);
    // Handle of the in-flight debounce+RPC task so the next keystroke can
    // `.cancel()` it before spawning a new one — only one search is ever
    // in flight (same pattern as search_palette/overlay.rs, #126).
    let mut current_task = use_signal(|| Option::<Task>::None);

    let search_url = server_url.clone();
    let search_exclude = target_uuid.clone();
    let mut run_search = move |q: String| {
        if let Some(prev) = current_task.write().take() {
            prev.cancel();
        }
        let url = search_url.clone();
        let exclude = search_exclude.clone();
        let task = spawn(async move {
            async_sleep_ms(150).await;
            if q.trim().is_empty() {
                results.set(Vec::new());
                error.set(None);
                return;
            }
            // The cancel above makes stale completions unlikely, but a
            // response already past the await when the next keystroke
            // lands could still race — drop anything (success *or*
            // failure) that no longer matches what the user typed.
            // `merge_candidates` (not `search_ebooks`): the candidate
            // pool must span both libraries — the typical merge pairs
            // an ebook with an audiobook.
            match data::merge_candidates(&url, &q).await {
                Ok(books) => {
                    if query() == q {
                        error.set(None);
                        results.set(
                            books
                                .into_iter()
                                .filter(|b| {
                                    b.unique_identifier.as_deref() != Some(exclude.as_str())
                                })
                                .take(8)
                                .collect(),
                        );
                    }
                }
                Err(e) => {
                    if query() == q {
                        error.set(Some(e.to_string()));
                    }
                }
            }
        });
        current_task.set(Some(task));
    };

    let merge_url = server_url.clone();
    let merge_target = target_uuid.clone();
    let do_merge = move |_| {
        // Re-entrancy guard: a double-click could land before the
        // disabled state re-renders.
        if busy() {
            return;
        }
        let Some(source) = selected() else { return };
        let Some(source_uuid) = source.unique_identifier.clone() else {
            return;
        };
        let url = merge_url.clone();
        let target = merge_target.clone();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match data::merge_books(&url, &source_uuid, &target).await {
                Ok(res) => on_merged.call(res),
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e.to_string()));
                }
            }
        });
    };

    rsx! {
        div {
            class: "author-photo-modal-backdrop",
            role: "dialog",
            aria_modal: "true",
            aria_label: "Merge with another book",
            // No dismissal while a merge is executing — closing would
            // hide the outcome (the request itself keeps running).
            onclick: move |_| {
                if !busy() {
                    on_close.call(());
                }
            },
            div {
                class: "author-photo-modal bd-merge-modal",
                onclick: move |evt| evt.stop_propagation(),
                div { class: "author-photo-modal__head",
                    h2 { class: "author-photo-modal__title", "Merge with\u{2026}" }
                    p { class: "subtitle author-photo-modal__sub",
                        "Pick the duplicate to fold into \u{201c}{target_title}\u{201d}."
                    }
                }
                if let Some(src) = selected() {
                    {render_confirm(src, target_title.clone(), busy(), do_merge, move |_| selected.set(None))}
                } else {
                    section { class: "author-photo-modal__section",
                        label { class: "label", r#for: "bd-merge-search", "Search your library" }
                        input {
                            id: "bd-merge-search",
                            class: "me-input",
                            "data-testid": "merge-search",
                            r#type: "search",
                            placeholder: "Title, author\u{2026}",
                            value: "{query}",
                            autofocus: true,
                            oninput: move |evt| {
                                let q = evt.value();
                                query.set(q.clone());
                                run_search(q);
                            },
                        }
                        ul { class: "bd-merge-results",
                            for book in results() {
                                {render_candidate(book, selected)}
                            }
                        }
                    }
                }
                if let Some(msg) = error() {
                    p { role: "alert", class: "bd-merge-error", "{msg}" }
                }
                div { class: "bd-merge-foot",
                    button {
                        class: "btn ghost sm",
                        "data-testid": "merge-cancel",
                        disabled: busy(),
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                }
            }
        }
    }
}

// Debounce sleep, platform-gated like search_palette/overlay.rs: the
// dialog compiles for web (WASM) and server (SSR) targets.
#[cfg(feature = "web")]
async fn async_sleep_ms(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}

#[cfg(all(not(feature = "web"), feature = "server"))]
async fn async_sleep_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

/// One search hit: title, author line, and format badges.
fn render_candidate(book: EbookMetadata, mut selected: Signal<Option<EbookMetadata>>) -> Element {
    let title = book.title.clone().unwrap_or_else(|| book.filename.clone());
    let authors = book
        .creators
        .iter()
        .map(|c| c.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let formats = book.formats.join(" · ").to_uppercase();
    let key = book.unique_identifier.clone().unwrap_or_default();
    rsx! {
        li { key: "{key}",
            button {
                class: "btn ghost bd-merge-candidate",
                "data-testid": "merge-candidate",
                onclick: move |_| selected.set(Some(book.clone())),
                span { class: "bd-merge-candidate-title", "{title}" }
                span { class: "mono bd-merge-candidate-meta",
                    if authors.is_empty() { "{formats}" } else { "{authors} \u{00b7} {formats}" }
                }
            }
        }
    }
}

/// Confirm panel once a candidate is picked.
fn render_confirm(
    source: EbookMetadata,
    target_title: String,
    busy: bool,
    on_confirm: impl FnMut(()) + 'static,
    on_back: impl FnMut(()) + 'static,
) -> Element {
    let mut on_confirm = on_confirm;
    let mut on_back = on_back;
    let source_title = source
        .title
        .clone()
        .unwrap_or_else(|| source.filename.clone());
    rsx! {
        section { class: "author-photo-modal__section",
            p { class: "bd-merge-confirm-copy",
                strong { "\u{201c}{source_title}\u{201d}" }
                " will be merged into "
                strong { "\u{201c}{target_title}\u{201d}" }
                ". Its files, tags, and reading progress move here; the other entry disappears. This can be undone."
            }
            div { class: "bd-merge-confirm-actions",
                button {
                    class: "btn primary",
                    "data-testid": "merge-confirm",
                    disabled: busy,
                    onclick: move |_| on_confirm(()),
                    if busy { "Merging\u{2026}" } else { "Merge" }
                }
                button {
                    class: "btn ghost",
                    "data-testid": "merge-back",
                    disabled: busy,
                    onclick: move |_| on_back(()),
                    "Back"
                }
            }
        }
    }
}
