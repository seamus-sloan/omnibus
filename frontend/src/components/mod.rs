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
pub use bottom_nav::BottomNav as Nav;

mod format_switcher;
pub use format_switcher::FormatSwitcher;

pub mod atrium;

// F1.11 follow-up: hover-overlay "edit photo" affordance + modal with
// three actions (paste URL, upload file, scan Open Library). Mounted by
// the author detail hero only — the `/authors` index renders cached
// photos read-only. Lives in components/ rather than pages/ so the
// modal can be lifted to other surfaces (e.g. an admin bulk-edit view)
// without duplicating the form.
pub mod author_photo_edit;

#[cfg(not(feature = "mobile"))]
pub mod search_palette;

// F0.5 / issue #69 worker-progress indicator. Mounted on the settings
// page today; the data plane is generic so future surfaces (e.g. the
// F5.9 library-cleanup detection trigger) can mount the same primitive
// without changes here.
#[cfg(not(feature = "mobile"))]
pub mod worker_status;

#[cfg(not(feature = "mobile"))]
mod user_menu;

// Auth-page primitives — used by the login / register / first-run /
// recovery screens on every target. Stays platform-agnostic so the same
// markup ships under web SSR, web WASM, and mobile native.
pub mod auth;
