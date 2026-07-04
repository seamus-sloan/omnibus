//! Shared API types between the `omnibus` server and `omnibus-mobile`
//! client. Kept free of Dioxus and transport-layer dependencies so both
//! `#[server]` functions (web) and `reqwest` calls (mobile) can depend on
//! it. Types are organized by domain into submodules and re-exported flat
//! from the crate root as `omnibus_shared::Foo`.

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
pub mod upload;
pub mod view_prefs;
pub mod worker;

/// Maximum byte length of an author photo source URL.
pub const AUTHOR_PHOTO_URL_MAX_LEN: usize = 2048;

/// Maximum byte length of a stored Hardcover API key (Bearer token).
pub const HARDCOVER_API_KEY_MAX_LEN: usize = 2048;

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
pub use upload::*;
pub use view_prefs::*;
pub use worker::*;
