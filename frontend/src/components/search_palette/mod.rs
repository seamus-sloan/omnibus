//! Command-palette / Spotlight-style search overlay. Replaces the inline
//! nav search input with a trigger button in the top nav; clicking it (or
//! pressing `⌘K` / `Ctrl+K`) opens a floating palette with debounced FTS5
//! search and grouped results (Books, Authors, Series, Tags, Inside text).
//! Mounted by `TopNav`; web-only via `cfg(not(feature = "mobile"))`.

use dioxus::prelude::*;

mod keyboard;
mod model;
mod overlay;
mod results;

use overlay::SpOverlay;

/// Whether the search palette overlay is open. Registered at the App level
/// via `use_context_provider` so both the trigger button and the global
/// `⌘K` shortcut can toggle it.
//
// Hydration safety: the palette starts closed (`PaletteOpen = false`) and
// `SpOverlay` only renders when open, so SSR and WASM agree on initial DOM
// — no hydration mismatch. The `⌘K` listener fires only under
// `feature = "web"`.
#[derive(Copy, Clone, PartialEq)]
pub struct PaletteOpen(pub Signal<bool>);

/// Top-level host: renders the trigger button and (when open) the overlay.
/// Mount this in `TopNav` in place of the old `NavSearch`.
//
// Component tree:
//   SearchPaletteHost          — mounted in TopNav, replaces NavSearch
//   ├─ SpTriggerButton         — search button (icon + "Search" + ⌘K kbd hint)
//   └─ (open) SpOverlay        — portal: scrim + panel
//              ├─ SpInput       — autofocused serif italic input
//              ├─ SpMeta        — "5 results · 18ms"
//              ├─ SpResultsList — scrollable grouped results
//              └─ SpFooter      — keyboard hints + "fts5 · ranked"
#[component]
pub fn SearchPaletteHost() -> Element {
    let open = use_context::<PaletteOpen>();

    // Hotkey lives at App scope (see `use_palette_global_shortcut`):
    // `TopNav` re-mounts on every route, so registering here would leak
    // a fresh listener per visit and flip the signal multiple times per
    // press.

    rsx! {
        SpTriggerButton { open }
        if open.0() {
            SpOverlay { open }
        }
    }
}

// ── Trigger button ───────────────────────────────────────────────

/// Pill-shaped search button in the nav: search icon + "Search" + ⌘K hint.
#[component]
fn SpTriggerButton(open: PaletteOpen) -> Element {
    let mut open = open;
    rsx! {
        button {
            class: "sp-trigger",
            "data-testid": "search-trigger",
            r#type: "button",
            onclick: move |_| open.0.set(true),
            // Search icon (SVG magnifying glass)
            svg {
                class: "sp-trigger-icon",
                width: "15",
                height: "15",
                view_box: "0 0 24 24",
                fill: "none",
                stroke: "currentColor",
                stroke_width: "2",
                stroke_linecap: "round",
                stroke_linejoin: "round",
                circle { cx: "11", cy: "11", r: "8" }
                line { x1: "21", y1: "21", x2: "16.65", y2: "16.65" }
            }
            span { "Search" }
            kbd { class: "sp-trigger-kbd", "⌘K" }
        }
    }
}

// ── Global ⌘K shortcut (web only) ───────────────────────────────

/// Register a global `keydown` listener that toggles the palette on `⌘K`
/// (Mac) or `Ctrl+K` (other platforms).
///
/// Must be called from a component that mounts **exactly once** for the
/// app's lifetime (i.e. `App`), not from a component that re-mounts on
/// route changes. `use_hook` only guarantees "once per component
/// instance", so a re-mounting host would accumulate listeners — each
/// press would then toggle the signal N times and effectively no-op on
/// the second route onward.
#[cfg(feature = "web")]
pub fn use_palette_global_shortcut() {
    let mut open = use_context::<PaletteOpen>();
    use_hook(move || {
        use wasm_bindgen::prelude::*;

        let closure = Closure::wrap(Box::new(move |evt: web_sys::KeyboardEvent| {
            let is_cmd_k = (evt.meta_key() || evt.ctrl_key()) && evt.key() == "k";
            if is_cmd_k {
                evt.prevent_default();
                let current = open.0();
                open.0.set(!current);
            }
        }) as Box<dyn FnMut(_)>);

        if let Some(window) = web_sys::window() {
            let _ = window
                .add_event_listener_with_callback("keydown", closure.as_ref().unchecked_ref());
        }

        // Leak the closure so it lives for the app lifetime. The shortcut
        // is registered once (use_hook guarantees this) and never removed —
        // acceptable for a single-page app.
        closure.forget();
    });
}

/// Non-web no-op so the App component can call this unconditionally on
/// non-mobile targets (SSR + native) without dragging in `web_sys`.
#[cfg(all(not(feature = "web"), not(feature = "mobile")))]
pub fn use_palette_global_shortcut() {}
