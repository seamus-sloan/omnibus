//! Shared SSR render helpers for frontend tests.
//!
//! `render` wraps `dioxus::ssr::render_element` for components that render to
//! HTML without a live runtime at the call site; `render_in_vdom` drives a
//! throwaway `VirtualDom` for the cases that must construct a `Signal` (or
//! otherwise touch the runtime) before rendering. Both exist only under the
//! `server` feature, where `dioxus::ssr` is available and where these tests
//! count toward llvm-cov.
//!
//! **Testing `#[cfg(feature = "mobile")]`-only markup** needs both cfgs at
//! once — `mobile` to compile the gated component, `server` for this
//! module's `dioxus::ssr` backing — so those tests run under
//! `cargo test -p omnibus-frontend --features mobile,server`, a combination
//! neither CI feature-matrix leg (`frontend` / `frontend-mobile`) exercises
//! on its own. Gate such a test module on `#[cfg(all(test, feature =
//! "server"))]` inside a file the `mobile` feature already pulls in (e.g. a
//! submodule of `pages::listen::mobile`, or a `#[cfg(feature = "mobile")]`
//! function's own module) — the surrounding file's mobile-only compilation
//! is what supplies the other half. See `pages::listen::mobile::sheets` and
//! `components::format_switcher::kindle` for worked examples: build a
//! zero-prop harness component (or a bare `Element` for a hookless
//! function), render it, and assert on substrings of the HTML.

use dioxus::prelude::*;

/// SSR-render an rsx `Element` to an HTML string. The workhorse for
/// render-smoke tests: pass `rsx! { SomeComponent { ..props } }` and assert on
/// substrings of the returned markup.
pub fn render(element: Element) -> String {
    dioxus::ssr::render_element(element)
}

/// SSR-render a zero-prop component inside a real `VirtualDom`, for the
/// components whose body constructs a `Signal` (`Signal::new`) or otherwise
/// needs a live runtime at mount. Returns the rendered HTML after one rebuild.
pub fn render_in_vdom(app: fn() -> Element) -> String {
    let mut dom = VirtualDom::new(app);
    dom.rebuild_in_place();
    dioxus::ssr::render(&dom)
}
