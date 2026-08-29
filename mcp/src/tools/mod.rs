//! Tool implementations, one module per family. Each module contributes a
//! `#[tool_router(router = <name>)]` impl block on
//! [`crate::server::OmnibusMcp`]; `OmnibusMcp::new` combines the routers.
//! Read tools live in [`read`], the check-in / wishlist / physical-collection
//! family in [`checkin`], shelf authoring in [`shelves`]; later issues add
//! further families as sibling modules, subject to the allowlist policy in
//! [`crate::client`].

pub mod checkin;
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
