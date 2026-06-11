//! Shared API types between the `omnibus` server and `omnibus-mobile` client.
//!
//! Keep this crate free of Dioxus and transport-layer dependencies so both
//! `#[server]` functions (web) and `reqwest` calls (mobile) can depend on it
//! without dragging in platform-specific trees.
//!
//! Types are organized by domain into submodules and re-exported flat from
//! the crate root, so downstream callers keep using `omnibus_shared::Foo`
//! regardless of where `Foo` actually lives.

pub mod audiobook;
pub mod auth;
pub mod discovery;
pub mod ebook;
pub mod image_format;
pub mod merge;
pub mod progress;
pub mod settings;
pub mod view_prefs;
pub mod worker;

pub use audiobook::*;
pub use auth::*;
pub use discovery::*;
pub use ebook::*;
pub use image_format::detect_image_format;
pub use merge::*;
pub use progress::*;
pub use settings::*;
pub use view_prefs::*;
pub use worker::*;
