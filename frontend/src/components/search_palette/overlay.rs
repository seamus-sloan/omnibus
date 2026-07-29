//! The floating palette panel itself: dark scrim, centered panel with
//! autofocused input, meta line, grouped result list, and a footer with
//! keyboard hints. Owns the debounced FTS5 search task (cancel-on-keystroke),
//! using the shared `platform_sleep`/`focus_after_paint` interop helpers.

use dioxus::core::Task;
use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::PaletteResults;

use super::keyboard::{make_keydown_handler, KeyboardContext};
use super::model::{build_flat_items, plural};
use super::results::SpResultsList;
use super::PaletteOpen;
use crate::focus_after_paint::focus_after_paint;
use crate::platform_sleep::async_sleep_ms;
use crate::{data, use_server_url};

/// Floating overlay: dark scrim + centered panel with input, results, footer.
#[component]
pub(super) fn SpOverlay(open: PaletteOpen) -> Element {
    let mut open = open;
    let server_url = use_server_url();
    let mut query = use_signal(String::new);
    let mut results = use_signal(|| Option::<PaletteResults>::None);
    let mut selected = use_signal(|| 0_usize);
    let mut loading = use_signal(|| false);
    // Set on a failed search, distinct from "no matches" (mirrors
    // `search_mobile.rs`'s `errored` signal for the same underlying call).
    let mut errored = use_signal(|| false);
    // Tracks whether the user has driven selection with arrow keys this
    // session. When false, pressing Enter navigates to the full-page
    // search results instead of drilling into `selected`.
    let mut has_navigated = use_signal(|| false);
    // #126: handle of the in-flight debounce+RPC task so the next keystroke
    // can `.cancel()` it before spawning a new one (instead of leaving N
    // sleeping spawns to race past the debounce and burn pool connections).
    let mut current_task = use_signal(|| Option::<Task>::None);
    let nav = use_navigator();

    // Build a flat list of selectable items for keyboard navigation.
    let flat_items = use_memo(move || build_flat_items(&results.read()));

    // Close the palette.
    let mut close = move || {
        open.0.set(false);
    };

    let on_keydown = make_keydown_handler(KeyboardContext {
        open,
        selected,
        has_navigated,
        flat_items,
        query,
        nav,
    });

    // Debounced search. Uses gloo_timers on web, tokio::time on server.
    // Each keystroke cancels the prior task (debounce sleep + RPC) before
    // spawning a new one — only one task is in flight at a time, so stale
    // requests don't race past the debounce or hit SQLite.
    let url = server_url.clone();
    let mut spawn_search = move |v: String| {
        if let Some(prev) = current_task.write().take() {
            prev.cancel();
        }
        let url = url.clone();
        let task = spawn(async move {
            async_sleep_ms(150).await;
            let trimmed = v.trim().to_string();
            if trimmed.is_empty() {
                results.set(None);
                loading.set(false);
                errored.set(false);
                return;
            }
            loading.set(true);
            errored.set(false);
            match data::search_palette(&url, &trimmed).await {
                Ok(r) => {
                    selected.set(0);
                    results.set(Some(r));
                }
                Err(_) => {
                    // `tracing` isn't linked under the `web` (WASM) feature,
                    // so the signal alone carries the failure to the UI.
                    results.set(None);
                    errored.set(true);
                }
            }
            loading.set(false);
        });
        current_task.set(Some(task));
    };

    let res = results.read();
    // Only project `selected` into row class names once the user has driven
    // selection with arrow keys. Otherwise the first row would render with
    // the "selected" highlight on every fresh query (since `selected`
    // resets to 0 after each search response), and pressing ArrowDown
    // would visually jump to index 1 instead of 0.
    let is_loading = loading();
    let is_errored = errored();

    let total = res
        .as_ref()
        .map(omnibus_shared::PaletteResults::total_count)
        .unwrap_or(0);
    let duration = res.as_ref().map(|r| r.duration_ms).unwrap_or(0);

    rsx! {
        div {
            class: "sp-scrim",
            "data-testid": "sp-scrim",
            onclick: move |_| close(),
            tabindex: "-1",
            div {
                class: "sp-panel",
                "data-testid": "sp-panel",
                role: "dialog",
                aria_label: "Search palette",
                aria_modal: "true",
                // Stop clicks inside the panel from closing the scrim.
                onclick: move |evt| evt.stop_propagation(),
                onkeydown: on_keydown,

                SpInputRow {
                    query,
                    is_loading,
                    on_input: move |v: String| {
                        query.set(v.clone());
                        spawn_search(v);
                        // Typing reverts to "Enter goes to /search" until
                        // the user re-engages arrow-key navigation.
                        has_navigated.set(false);
                        selected.set(0);
                    },
                }

                // Meta line
                if res.is_some() {
                    div { class: "sp-meta", "data-testid": "sp-result-count",
                        "{total} result{plural(total)} · {duration}ms"
                    }
                }

                // Distinct from "no matches": a fetch failure, not an empty
                // result set.
                if is_errored {
                    p { role: "alert", class: "error", "data-testid": "sp-error",
                        "Couldn\u{2019}t run that search. Check your connection and try again."
                    }
                }

                SpResultsList {
                    results,
                    flat_items,
                    selected,
                    has_navigated,
                    open,
                }

                // Footer
                SpFooter {}
            }
        }
    }
}

/// Search-icon + autofocused query input, with the debounce/RPC dispatch
/// delegated to `on_input` and the loading spinner shown while a search runs.
#[component]
fn SpInputRow(query: Signal<String>, is_loading: bool, on_input: EventHandler<String>) -> Element {
    rsx! {
        div { class: "sp-input-wrap",
            svg {
                class: "sp-input-icon",
                width: "18",
                height: "18",
                view_box: "0 0 24 24",
                fill: "none",
                stroke: "currentColor",
                stroke_width: "2",
                stroke_linecap: "round",
                stroke_linejoin: "round",
                circle { cx: "11", cy: "11", r: "8" }
                line { x1: "21", y1: "21", x2: "16.65", y2: "16.65" }
            }
            input {
                class: "sp-input",
                "data-testid": "sp-input",
                r#type: "text",
                placeholder: "Search books, authors, series, tags…",
                autofocus: true,
                // `autofocus` only fires on initial page load.
                // `onmounted` programmatically focuses the input
                // when the overlay is dynamically rendered (⌘K).
                // Uses `requestAnimationFrame` so the browser has
                // finished layout before we `.focus()` — without
                // that delay the focus call lands on an element
                // that isn't yet painted and the caret never
                // appears (this is the timing reason prior
                // attempts with `set_focus(true)`/`spawn` failed).
                onmounted: move |evt: MountedEvent| {
                    focus_after_paint(&evt);
                },
                value: "{query}",
                oninput: move |evt| on_input.call(evt.value()),
            }
            if is_loading {
                span { class: "sp-spinner", "…" }
            }
        }
    }
}

#[component]
fn SpFooter() -> Element {
    rsx! {
        div { class: "sp-footer",
            div { class: "sp-footer-keys",
                kbd { "↑↓" }
                span { " navigate" }
                kbd { "⏎" }
                span { " open" }
                kbd { "esc" }
                span { " close" }
            }
            div { class: "sp-footer-engine",
                "fts5 · ranked by relevance"
            }
        }
    }
}
