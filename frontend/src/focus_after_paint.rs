//! Shared "focus this panel after paint" helper, used by every dismissible
//! overlay/menu whose `onmounted` handler needs the mounted element to hold
//! focus (so `onkeydown` picks up ESC without a prior click).

use dioxus::prelude::*;

/// Focus the mounted element on the next animation frame.
///
/// The `requestAnimationFrame` hop is required: calling `.focus()`
/// synchronously inside `onmounted` (or from a `spawn`ed future) lands
/// before the element is laid out, so the caret/focus ring never appears.
/// Scheduling the focus for the next frame lets the browser finish layout
/// first.
#[cfg(feature = "web")]
pub fn focus_after_paint(evt: &MountedEvent) {
    use dioxus::web::WebEventExt;
    use wasm_bindgen::prelude::*;

    let Some(element) = evt.try_as_web_event() else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    // `Closure::once_into_js` hands the callback to JS and frees the Rust
    // closure (and the captured `element`) after the browser fires it once,
    // so repeatedly mounting/unmounting the panel doesn't accumulate leaks.
    let cb = Closure::once_into_js(move || {
        if let Some(html_el) = element.dyn_ref::<web_sys::HtmlElement>() {
            let _ = html_el.focus();
        }
    });
    let _ = window.request_animation_frame(cb.unchecked_ref());
}

/// Non-web stub: SSR never paints the panel and native shells don't drive
/// these overlays through a browser focus ring, so there is nothing to
/// focus. Defined so an `onmounted` handler can call `focus_after_paint`
/// unconditionally (rule 07: hydration parity — keep cfg gates out of rsx
/// bodies).
#[cfg(not(feature = "web"))]
pub fn focus_after_paint(_evt: &MountedEvent) {}
