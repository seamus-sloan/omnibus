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
//! on its own. The exact `#[cfg(...)]` to write on the test module depends
//! on whether the *file* is already mobile-only:
//!
//! - **A mobile-only file** — one whose module root already sits behind
//!   `#![cfg(feature = "mobile")]` (e.g. `pages::listen::mobile` and its
//!   submodules) — only needs `server` spelled out on the test module itself;
//!   the file-level gate already supplies the `mobile` half:
//!   ```ignore
//!   #[cfg(feature = "server")]
//!   mod render {
//!       use crate::test_support::render_in_vdom;
//!       // ... build a harness, call render_in_vdom, assert on the HTML
//!   }
//!   ```
//!   Worked example: `pages::listen::mobile::sheets`.
//! - **A shared file**, where only some items are `#[cfg(feature =
//!   "mobile")]` and the rest of the file compiles on every target (e.g.
//!   `components::format_switcher::kindle`, which also has a non-mobile
//!   `SendToKindleButton`) — the test module needs both cfgs spelled out
//!   explicitly, since nothing upstream of it implies `mobile`:
//!   ```ignore
//!   #[cfg(all(test, feature = "mobile", feature = "server"))]
//!   mod mobile_render_tests {
//!       use crate::test_support::render;
//!       // ... render the mobile-only fn's Element, assert on the HTML
//!   }
//!   ```
//!   Worked example: `components::format_switcher::kindle`.
//!
//! Either way: build a zero-prop harness component (or a bare `Element` for
//! a hookless function), render it, and assert on substrings of the HTML.

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
