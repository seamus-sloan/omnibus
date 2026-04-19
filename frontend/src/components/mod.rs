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

// The default (web) variant must compile under the `server` feature too so
// SSR markup matches what the WASM client expects to hydrate. Otherwise
// dioxus's hydration walker fails to locate dynamic text/event nodes and
// throws "Cannot set properties of undefined".
#[cfg(not(feature = "mobile"))]
mod top_nav;
#[cfg(not(feature = "mobile"))]
pub use top_nav::TopNav as Nav;

#[cfg(feature = "mobile")]
mod bottom_nav;
#[cfg(feature = "mobile")]
pub use bottom_nav::bottom_nav as Nav;
