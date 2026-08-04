//! Command-palette / Spotlight-style search overlay. Replaces the inline
//! nav search input with a trigger button in the top nav; clicking it (or
//! pressing `⌘K` / `Ctrl+K`) opens a floating palette with debounced FTS5
//! search and grouped results. The trigger mounts in `TopNav`, the overlay
//! at the `ScreenLayout` root; web-only via `cfg(not(feature = "mobile"))`.

use dioxus::prelude::*;

mod keyboard;
mod model;
mod overlay;
mod results;

use crate::components::glyphs::search_glyph;
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

/// The search button in the top nav. Mount this in `TopNav` in place of
/// the old `NavSearch`.
///
/// The overlay is deliberately **not** a child of this component — see
/// [`SearchPaletteOverlay`].
//
// Hotkey lives at App scope (see `use_palette_global_shortcut`): `TopNav`
// re-mounts on every route, so registering here would leak a fresh
// listener per visit and flip the signal multiple times per press.
#[component]
pub fn SearchPaletteTrigger() -> Element {
    let open = use_context::<PaletteOpen>();
    rsx! {
        SpTriggerButton { open }
    }
}

/// The palette overlay (scrim + panel), mounted once at the `ScreenLayout`
/// root rather than next to the trigger.
///
/// Root placement is load-bearing, not cosmetic: `.atrium-topbar` sets
/// `backdrop-filter`, which makes it the containing block for
/// `position: fixed` descendants. Rendered inside the nav, the scrim's
/// `inset: 0` resolved to the topbar's box — it dimmed only the header
/// strip and clicks below it never reached the scrim, so click-outside
/// couldn't close. Mounting at the root also keeps `⌘K` working on
/// `/settings`, where `TopNav` hides the trigger.
//
// Component tree:
//   SearchPaletteOverlay        — mounted in ScreenLayout
//   └─ (open) SpOverlay         — scrim + panel
//              ├─ SpInput       — autofocused serif italic input
//              ├─ SpMeta        — "5 results · 18ms"
//              ├─ SpResultsList — scrollable grouped results
//              └─ SpFooter      — keyboard hints + "fts5 · ranked"
#[component]
pub fn SearchPaletteOverlay() -> Element {
    let open = use_context::<PaletteOpen>();
    rsx! {
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
            {search_glyph(15, "sp-trigger-icon", false)}
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

#[cfg(all(test, feature = "server"))]
mod tests {
    use super::*;
    use crate::test_support::render_in_vdom;

    /// Provides the `PaletteOpen` context both public components read, then
    /// mounts them side by side, starting closed — mirrors how `App`
    /// provides the context and `TopNav`/`ScreenLayout` mount the trigger
    /// and overlay separately. The `open` branch renders `SpOverlay`, whose
    /// body calls `use_navigator()`, which needs a live `Router` this test
    /// doesn't mount (the same reason `TopNav`'s own render is untested;
    /// `book_detail/highlights/tests.rs` does mount one for `Link` coverage,
    /// but wiring that harness here is out of scope) — so only the closed
    /// state (the SSR/first-paint default, rule 07) is exercised here.
    fn closed_harness() -> Element {
        use_context_provider(|| PaletteOpen(Signal::new(false)));
        rsx! {
            SearchPaletteTrigger {}
            SearchPaletteOverlay {}
        }
    }

    #[test]
    fn search_palette_trigger_renders_the_trigger_button() {
        let html = render_in_vdom(closed_harness);
        assert!(html.contains("data-testid=\"search-trigger\""));
        assert!(html.contains("Search"));
    }

    #[test]
    fn search_palette_overlay_renders_nothing_while_closed() {
        let html = render_in_vdom(closed_harness);
        assert!(!html.contains("sp-scrim"));
        assert!(!html.contains("sp-panel"));
    }

    // The `web` arm registers a real `keydown` listener via `web_sys` and
    // needs the wasm32 target, so only the non-web no-op is exercisable
    // here — same blind spot `contexts::use_can_upload`'s test already has.
    // It's still worth pinning: `App` calls this unconditionally on every
    // non-mobile target, so a panic here would take the whole app down.
    #[cfg(not(feature = "mobile"))]
    #[test]
    fn use_palette_global_shortcut_non_web_fallback_is_a_no_op() {
        use_palette_global_shortcut();
    }
}
