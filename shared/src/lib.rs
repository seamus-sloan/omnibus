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
pub mod bookmark;
pub mod discovery;
pub mod ebook;
pub mod highlight;
pub mod image_format;
pub mod journal;
pub mod merge;
pub mod progress;
pub mod ratings;
pub mod settings;
pub mod shelves;
pub mod suggestion;
pub mod view_prefs;
pub mod worker;

/// Maximum byte length of an author photo source URL.
pub const AUTHOR_PHOTO_URL_MAX_LEN: usize = 2048;

pub use audiobook::*;
pub use auth::*;
pub use bookmark::*;
pub use discovery::*;
pub use ebook::*;
pub use highlight::*;
pub use image_format::detect_image_format;
pub use journal::*;
pub use merge::*;
pub use progress::*;
pub use ratings::*;
pub use settings::*;
pub use shelves::*;
pub use suggestion::*;
pub use view_prefs::*;
pub use worker::*;
