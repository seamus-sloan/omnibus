//! Tool implementations, one module per family: reads in [`read`], check-in /
//! wishlist / physical in [`checkin`], shelf authoring in [`shelves`],
//! metadata repair in [`metadata`], admin merge/undo in [`merge`], and the
//! chapter/content-search reads in [`content`]. Each is a `#[tool_router]`
//! impl block combined in `OmnibusMcp::new`, per [`crate::client`]'s allowlist.

pub mod checkin;
pub mod content;
pub mod merge;
pub mod metadata;
pub mod read;
pub mod shelves;

use rmcp::ErrorData;

/// Validate a model-supplied handle before it becomes one URL path segment.
///
/// The write plumbing asserts every request path against
/// [`crate::client::WRITE_ALLOWLIST`], so a value that splits into extra
/// segments (or an empty one) would trip that assert and panic the process.
/// Every path-bound handle this crate sends (book uuids) is a plain
/// `[A-Za-z0-9-]` string, so anything else is rejected as invalid params
/// before a path is built.
pub(crate) fn path_segment<'a>(value: &'a str, what: &str) -> Result<&'a str, ErrorData> {
    let well_formed =
        !value.is_empty() && value.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
    if well_formed {
        Ok(value)
    } else {
        Err(ErrorData::invalid_params(
            format!(
                "{what} must be a non-empty identifier (letters, digits, dashes): got {value:?}"
            ),
            None,
        ))
    }
}
