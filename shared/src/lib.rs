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
pub mod isbn;
pub mod journal;
pub mod logs;
pub mod merge;
pub mod metadata_lookup;
pub mod physical;
pub mod progress;
pub mod ratings;
pub mod scan;
pub mod settings;
pub mod shelves;
pub mod stats;
pub mod suggestion;
pub mod summary;
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

/// Maximum byte length of a `book_uuid` used purely as a lookup key (Kindle
/// send, shelf membership). A minted book uuid is a 36-byte hyphenated
/// UUIDv4; this leaves headroom for other id formats while still rejecting
/// abusive input before it reaches a DB lookup.
pub const BOOK_UUID_MAX_LEN: usize = 64;

/// Handler-level cap on a decoded search query's length, in bytes. Shared by
/// the REST search/palette handlers and their RPC counterparts so both
/// boundaries reject oversized input before any FTS5 call; the db layer still
/// applies its own char-count trim (`cap_query_len`) as a fallback.
pub const SEARCH_QUERY_MAX_LEN: usize = 1024;

/// `true` once `q` exceeds [`SEARCH_QUERY_MAX_LEN`] and should be rejected
/// before it reaches a search db call.
pub fn search_query_too_long(q: &str) -> bool {
    q.len() > SEARCH_QUERY_MAX_LEN
}

pub use audiobook::*;
pub use auth::*;
pub use bookmark::*;
pub use discovery::*;
pub use ebook::*;
pub use highlight::*;
pub use image_format::detect_image_format;
pub use isbn::*;
pub use journal::*;
pub use logs::*;
pub use merge::*;
pub use metadata_lookup::*;
pub use physical::*;
pub use progress::*;
pub use ratings::*;
pub use scan::*;
pub use settings::*;
pub use shelves::*;
pub use stats::*;
pub use suggestion::*;
pub use summary::*;
pub use upload::*;
pub use view_prefs::*;
pub use worker::*;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_query_too_long_rejects_over_cap_and_accepts_at_cap() {
        assert!(!search_query_too_long(&"a".repeat(SEARCH_QUERY_MAX_LEN)));
        assert!(search_query_too_long(&"a".repeat(SEARCH_QUERY_MAX_LEN + 1)));
    }
}
