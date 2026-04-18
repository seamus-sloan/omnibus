//! Nav is platform-specific: `TopNav` on web, `BottomNav` on mobile.
//!
//! The `web` and `mobile` features are **mutually exclusive** — enabling
//! both at once (e.g. because a workspace-wide `cargo build` unified them)
//! triggers a `compile_error!`. Build each platform crate with `cargo build
//! -p <crate>` so its exclusive feature set is used.

#[cfg(all(feature = "web", feature = "mobile"))]
compile_error!(
    "omnibus-frontend's `web` and `mobile` features are mutually exclusive. \
     A workspace-wide `cargo build`/`clippy` unifies features and enables both. \
     Build per-crate instead: `cargo build -p omnibus` for the server, \
     `cargo build -p omnibus-mobile` for the mobile app."
);

#[cfg(all(feature = "web", not(feature = "mobile")))]
mod top_nav;
#[cfg(all(feature = "web", not(feature = "mobile")))]
pub use top_nav::TopNav as Nav;

#[cfg(all(feature = "mobile", not(feature = "web")))]
mod bottom_nav;
#[cfg(all(feature = "mobile", not(feature = "web")))]
pub use bottom_nav::BottomNav as Nav;

// No platform feature — provide an empty Nav so `cargo doc` / `cargo check`
// still work without any feature flag.
#[cfg(not(any(feature = "web", feature = "mobile")))]
mod fallback {
    use dioxus::prelude::*;

    #[component]
    pub fn Nav() -> Element {
        rsx! { nav {} }
    }
}
#[cfg(not(any(feature = "web", feature = "mobile")))]
pub use fallback::Nav;
