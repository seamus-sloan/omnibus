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
pub mod logs;
pub mod merge;
pub mod physical;
pub mod progress;
pub mod ratings;
pub mod settings;
pub mod shelves;
pub mod stats;
pub mod suggestion;
pub mod upload;
pub mod view_prefs;
pub mod worker;

/// Maximum byte length of an author photo source URL.
pub const AUTHOR_PHOTO_URL_MAX_LEN: usize = 2048;

/// Maximum byte length of a stored Hardcover API key (Bearer token).
pub const HARDCOVER_API_KEY_MAX_LEN: usize = 2048;

/// Maximum byte length of an SMTP host / username field.
pub const SMTP_FIELD_MAX_LEN: usize = 255;

/// Maximum byte length of an SMTP password.
pub const SMTP_PASSWORD_MAX_LEN: usize = 1024;

/// Maximum byte length of an email address field — an SMTP `from` or a user's
/// Kindle destination. RFC 5321 caps a path at 256 bytes; 320 is the
/// commonly-cited practical upper bound for `local@domain`.
pub const EMAIL_MAX_LEN: usize = 320;

pub use audiobook::*;
pub use auth::*;
pub use bookmark::*;
pub use discovery::*;
pub use ebook::*;
pub use highlight::*;
pub use image_format::detect_image_format;
pub use journal::*;
pub use logs::*;
pub use merge::*;
pub use physical::*;
pub use progress::*;
pub use ratings::*;
pub use settings::*;
pub use shelves::*;
pub use stats::*;
pub use suggestion::*;
pub use upload::*;
pub use view_prefs::*;
pub use worker::*;
