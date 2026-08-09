//! The floating palette panel itself: dark scrim, centered panel with
//! autofocused input, meta line, grouped result list, and a footer with
//! keyboard hints. Owns the debounced FTS5 search task (cancel-on-keystroke),
//! using the shared `platform_sleep`/`focus_after_paint` interop helpers.

use dioxus::core::Task;
use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::PaletteResults;

use super::keyboard::{make_keydown_handler, KeyboardContext};
use super::model::{build_flat_items, plural, FlatItem};
use super::results::SpResultsList;
use super::PaletteOpen;
use crate::components::glyphs::search_glyph;
use crate::focus_after_paint::focus_after_paint;
use crate::platform_sleep::async_sleep_ms;
use crate::{data, use_server_url};

/// Build the debounced search dispatcher. Each keystroke cancels the prior
/// in-flight task (debounce sleep + RPC) before spawning a new one — only
/// one task is in flight at a time, so stale requests don't race past the
/// debounce or hit SQLite (#126). Uses gloo_timers on web, tokio::time on
/// server.
fn build_search_dispatcher(
    server_url: String,
    mut results: Signal<Option<PaletteResults>>,
    mut selected: Signal<usize>,
    mut loading: Signal<bool>,
    mut errored: Signal<bool>,
    mut current_task: Signal<Option<Task>>,
) -> impl FnMut(String) + 'static {
    move |v: String| {
        if let Some(prev) = current_task.write().take() {
            prev.cancel();
        }
        let url = server_url.clone();
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
    }
}

/// `(total, duration_ms, has_results)` for the meta line, or `(0, 0, false)`
/// before the first response lands.
fn sp_result_summary(res: &Option<PaletteResults>) -> (usize, u64, bool) {
    let total = res
        .as_ref()
        .map(omnibus_shared::PaletteResults::total_count)
        .unwrap_or(0);
    let duration = res.as_ref().map(|r| r.duration_ms).unwrap_or(0);
    (total, duration, res.is_some())
}

/// Meta line ("N results · Xms") and the fetch-error notice. Distinct from
/// "no matches": `is_errored` means the request itself failed.
fn sp_meta_and_error(has_results: bool, total: usize, duration: u64, is_errored: bool) -> Element {
    rsx! {
        if has_results {
            div { class: "sp-meta", "data-testid": "sp-result-count",
                "{total} result{plural(total)} · {duration}ms"
            }
        }
        if is_errored {
            p { role: "alert", class: "error", "data-testid": "sp-error",
                "Couldn\u{2019}t run that search. Check your connection and try again."
            }
        }
    }
}

/// Local reactive state for one [`SpOverlay`] instance — a custom-hook-style
/// helper (the same pattern `chip_editor::use_chip_editor_state` uses) so the
/// component itself starts from the derived wiring instead of a wall of
/// signal declarations.
#[derive(Clone, Copy)]
struct SpOverlayState {
    query: Signal<String>,
    results: Signal<Option<PaletteResults>>,
    selected: Signal<usize>,
    loading: Signal<bool>,
    // Set on a failed search, distinct from "no matches" (mirrors
    // `search_mobile.rs`'s `errored` signal for the same underlying call).
    errored: Signal<bool>,
    // Tracks whether the user has driven selection with arrow keys this
    // session. When false, pressing Enter navigates to the full-page
    // search results instead of drilling into `selected`.
    has_navigated: Signal<bool>,
    // #126: handle of the in-flight debounce+RPC task — see
    // `build_search_dispatcher`.
    current_task: Signal<Option<Task>>,
}

fn use_sp_overlay_state() -> SpOverlayState {
    SpOverlayState {
        query: use_signal(String::new),
        results: use_signal(|| Option::<PaletteResults>::None),
        selected: use_signal(|| 0_usize),
        loading: use_signal(|| false),
        errored: use_signal(|| false),
        has_navigated: use_signal(|| false),
        current_task: use_signal(|| Option::<Task>::None),
    }
}

/// Derived wiring for [`SpOverlay`]: the flat selectable-item list plus the
/// close/keydown/search handlers the panel renders with. Split out purely to
/// keep [`SpOverlay`] itself under the file's line-count guidance — every
/// hook call here still runs during that component's own render, in the
/// same order every time.
#[allow(clippy::type_complexity)]
fn build_overlay_wiring(
    open: PaletteOpen,
    server_url: String,
    state: SpOverlayState,
) -> (
    Memo<Vec<FlatItem>>,
    impl FnMut() + 'static,
    impl FnMut(Event<KeyboardData>) + 'static,
    impl FnMut(String) + 'static,
) {
    let SpOverlayState {
        query,
        results,
        selected,
        loading,
        errored,
        has_navigated,
        current_task,
    } = state;
    let mut open_mut = open;
    let nav = use_navigator();
    let flat_items = use_memo(move || build_flat_items(&results.read()));

    let close = move || {
        open_mut.0.set(false);
    };
    let on_keydown = make_keydown_handler(KeyboardContext {
        open,
        selected,
        has_navigated,
        flat_items,
        query,
        nav,
    });
    let spawn_search = build_search_dispatcher(
        server_url,
        results,
        selected,
        loading,
        errored,
        current_task,
    );

    (flat_items, close, on_keydown, spawn_search)
}

/// Floating overlay: dark scrim + centered panel with input, results, footer.
#[component]
pub(super) fn SpOverlay(open: PaletteOpen) -> Element {
    let server_url = use_server_url();
    let state = use_sp_overlay_state();
    let SpOverlayState {
        mut query,
        results,
        mut selected,
        loading,
        errored,
        mut has_navigated,
        ..
    } = state;
    let (flat_items, mut close, on_keydown, mut spawn_search) =
        build_overlay_wiring(open, server_url, state);

    let res = results.read();
    let is_loading = loading();
    let is_errored = errored();
    let (total, duration, has_results) = sp_result_summary(&res);

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

                {sp_meta_and_error(has_results, total, duration, is_errored)}

                // `has_navigated` gates whether `selected` is projected into
                // row class names: until the user drives arrow-key selection,
                // the first row would otherwise render "selected" on every
                // fresh query (it resets to 0 after each response).
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
            {search_glyph(18, "sp-input-icon", false)}
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
