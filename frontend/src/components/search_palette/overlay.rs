//! The floating palette panel itself: dark scrim, centered panel with
//! autofocused input, meta line, grouped result list, and a footer with
//! keyboard hints. Owns the debounced FTS5 search task (cancel-on-keystroke),
//! using the shared `platform_sleep`/`focus_after_paint` interop helpers.

use std::cell::RefCell;
use std::rc::Rc;

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
/// close/keydown/search handlers the panel renders with. Every hook call
/// here still runs during [`SpOverlay`]'s own render, in the same order
/// every time — required since Dioxus hooks must stay unconditional and
/// order-stable regardless of which function body they're written in.
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
///
/// Real DOM focus lands a frame after mount (see [`focus_after_paint`]), so
/// a click-then-immediately-type user's first keystrokes would otherwise
/// land on whatever had focus before the overlay opened and vanish (#1908).
/// `install_prefocus_key_buffer` closes that window: a temporary
/// `document`-level `keydown` listener folds keystrokes into `query` until
/// real focus lands, at which point `remove_prefocus_key_buffer` detaches
/// it so ordinary typing goes through `oninput` as usual.
#[component]
fn SpInputRow(query: Signal<String>, is_loading: bool, on_input: EventHandler<String>) -> Element {
    let buffer: PrefocusKeyBufferHolder = use_hook(Rc::default);
    // Safety net: detach if the overlay closes (e.g. Escape) before real
    // focus ever lands, so a stale listener doesn't outlive this instance.
    use_drop({
        let buffer = Rc::clone(&buffer);
        move || remove_prefocus_key_buffer(&buffer)
    });

    let mount_buffer = Rc::clone(&buffer);
    let focus_buffer = Rc::clone(&buffer);
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
                    remove_prefocus_key_buffer(&mount_buffer);
                    *mount_buffer.borrow_mut() = install_prefocus_key_buffer(query, on_input);
                },
                // Real focus has landed — the buffer's job is done.
                onfocus: move |_| remove_prefocus_key_buffer(&focus_buffer),
                value: "{query}",
                oninput: move |evt| on_input.call(evt.value()),
            }
            if is_loading {
                span { class: "sp-spinner", "…" }
            }
        }
    }
}

/// Handle for the temporary pre-focus `keydown` buffer — see
/// [`install_prefocus_key_buffer`]. Held in a [`PrefocusKeyBufferHolder`]
/// until [`remove_prefocus_key_buffer`] detaches it.
#[cfg(feature = "web")]
struct PrefocusKeyBuffer {
    document: web_sys::Document,
    closure: wasm_bindgen::closure::Closure<dyn FnMut(web_sys::KeyboardEvent)>,
}

/// Non-web stub: SSR and native shells don't drive this browser focus race
/// (mirrors `focus_after_paint`, rule 07).
#[cfg(not(feature = "web"))]
struct PrefocusKeyBuffer;

/// Shared slot the `onmounted`/`onfocus`/unmount call sites all reach
/// through to install and later detach the buffer.
type PrefocusKeyBufferHolder = Rc<RefCell<Option<PrefocusKeyBuffer>>>;

/// Install a temporary `document`-level `keydown` listener that folds
/// printable keystrokes (and Backspace) into `query`/`on_input` while real
/// DOM focus is still in flight. Returns `None` if `window`/`document` is
/// unavailable — the same best-effort shape as `focus_after_paint`.
#[cfg(feature = "web")]
fn install_prefocus_key_buffer(
    query: Signal<String>,
    on_input: EventHandler<String>,
) -> Option<PrefocusKeyBuffer> {
    use wasm_bindgen::prelude::*;
    use wasm_bindgen::JsCast;

    let document = web_sys::window()?.document()?;
    let closure = Closure::wrap(Box::new(move |evt: web_sys::KeyboardEvent| {
        let Some(edit) = bufferable_key(&evt) else {
            return;
        };
        // Stop the key here, in the capture phase, before it ever reaches
        // whatever element still holds real focus — a bubble-phase
        // `prevent_default` alone would be too late to stop a focused
        // element's own `onkeydown` (e.g. a book tile's Space-to-open)
        // from already having fired during the same dispatch.
        evt.prevent_default();
        evt.stop_propagation();
        let mut v = query.peek().clone();
        match edit {
            Some(c) => v.push(c),
            None => {
                v.pop();
            }
        }
        on_input.call(v);
    }) as Box<dyn FnMut(_)>);
    document
        .add_event_listener_with_callback_and_bool(
            "keydown",
            closure.as_ref().unchecked_ref(),
            true,
        )
        .ok()?;
    Some(PrefocusKeyBuffer { document, closure })
}

#[cfg(not(feature = "web"))]
fn install_prefocus_key_buffer(
    _query: Signal<String>,
    _on_input: EventHandler<String>,
) -> Option<PrefocusKeyBuffer> {
    None
}

/// Detach a buffer installed by [`install_prefocus_key_buffer`], if any.
/// Idempotent — safe to call from both the input's real `onfocus` and the
/// row's unmount, whichever comes first.
fn remove_prefocus_key_buffer(holder: &PrefocusKeyBufferHolder) {
    let Some(buf) = holder.borrow_mut().take() else {
        return;
    };
    detach_prefocus_key_buffer(buf);
}

#[cfg(feature = "web")]
fn detach_prefocus_key_buffer(buf: PrefocusKeyBuffer) {
    use wasm_bindgen::JsCast;

    let _ = buf.document.remove_event_listener_with_callback_and_bool(
        "keydown",
        buf.closure.as_ref().unchecked_ref(),
        true,
    );
}

#[cfg(not(feature = "web"))]
fn detach_prefocus_key_buffer(_buf: PrefocusKeyBuffer) {}

/// `bufferable_key`'s decision, from primitive fields rather than a real
/// `web_sys::KeyboardEvent` — that type only constructs on a wasm32 target,
/// so pulling the classification out keeps it unit-testable on the host.
/// `Some(Some(c))` appends `c`, `Some(None)` deletes the last character
/// (Backspace), `None` skips the event — Enter/Tab/arrows/modified combos
/// are left for the palette's real `onkeydown` once focus lands for real.
/// Only reachable from `bufferable_key` (web) and the unit tests below — a
/// non-web, non-test build has neither caller, hence the `cfg`.
#[cfg(any(feature = "web", test))]
fn classify_prefocus_key(key: &str, ctrl: bool, meta: bool, alt: bool) -> Option<Option<char>> {
    if ctrl || meta || alt {
        return None;
    }
    if key == "Backspace" {
        return Some(None);
    }
    let mut chars = key.chars();
    match (chars.next(), chars.next()) {
        (Some(c), None) => Some(Some(c)),
        _ => None,
    }
}

/// Classify a `keydown` for the pre-focus buffer — see
/// [`classify_prefocus_key`] for the rules.
#[cfg(feature = "web")]
fn bufferable_key(evt: &web_sys::KeyboardEvent) -> Option<Option<char>> {
    classify_prefocus_key(&evt.key(), evt.ctrl_key(), evt.meta_key(), evt.alt_key())
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

#[cfg(test)]
mod tests {
    use super::classify_prefocus_key;

    #[test]
    fn classify_prefocus_key_appends_a_bare_printable_character() {
        assert_eq!(
            classify_prefocus_key("a", false, false, false),
            Some(Some('a'))
        );
    }

    #[test]
    fn classify_prefocus_key_allows_a_shifted_symbol() {
        // The browser already resolves `key` to the shifted glyph (e.g. "!"
        // for Shift+1), so there is no separate shift check here.
        assert_eq!(
            classify_prefocus_key("!", false, false, false),
            Some(Some('!'))
        );
    }

    #[test]
    fn classify_prefocus_key_deletes_the_last_character_on_backspace() {
        assert_eq!(
            classify_prefocus_key("Backspace", false, false, false),
            Some(None)
        );
    }

    #[test]
    fn classify_prefocus_key_ignores_ctrl_meta_and_alt_combos() {
        assert_eq!(classify_prefocus_key("a", true, false, false), None);
        assert_eq!(classify_prefocus_key("a", false, true, false), None);
        assert_eq!(classify_prefocus_key("a", false, false, true), None);
    }

    #[test]
    fn classify_prefocus_key_ignores_multi_character_key_names() {
        assert_eq!(classify_prefocus_key("Enter", false, false, false), None);
        assert_eq!(
            classify_prefocus_key("ArrowDown", false, false, false),
            None
        );
        assert_eq!(classify_prefocus_key("Tab", false, false, false), None);
    }
}
