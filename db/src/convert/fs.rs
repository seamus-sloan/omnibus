//! Filesystem layout for the format-conversion output cache: the cache
//! directory and the per-`(book_id, target_format)` output path. Mirrors
//! `crate::kepub::fs`. [`convert_path`] is the **only** place a
//! caller-supplied format token becomes a path component — every other
//! site that needs the cache path derives it from `convert_path`'s own
//! output rather than reformatting the token a second time.

use std::path::PathBuf;

use super::execute::ConvertError;

/// Longest format token [`validate_format_token`] accepts, mirroring
/// `omnibus_shared::sanitize_exclude_formats`' `[a-z0-9]{1,16}` cap on the
/// same class of caller-supplied format string.
const MAX_FORMAT_TOKEN_LEN: usize = 16;

/// Root directory for converted-format output files.
///
/// Override with `$OMNIBUS_CONVERT_DIR` (used verbatim); otherwise defaults
/// to `<$OMNIBUS_DATA_DIR>/convert` (data dir default `./data`).
pub fn convert_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("OMNIBUS_CONVERT_DIR") {
        return PathBuf::from(dir);
    }
    let base = std::env::var("OMNIBUS_DATA_DIR").unwrap_or_else(|_| "./data".into());
    PathBuf::from(base).join("convert")
}

/// Reject anything that isn't 1–[`MAX_FORMAT_TOKEN_LEN`] ASCII alphanumeric
/// characters.
///
/// `Task::ConvertFormat`'s `source_format`/`target_format` fields are plain
/// caller-supplied strings with no format allowlist upstream (there is no
/// wire route yet — see #948's PR notes — but the worker task itself must
/// not assume one ever validates its input before posting). A token that
/// isn't a bare extension — `../../etc/passwd`, `a/b`, an absolute path —
/// must never reach a `PathBuf::join`/`format!` that builds a filesystem
/// path or a temp-file name, or it can escape [`convert_dir`] entirely.
/// Rejecting anything but `[A-Za-z0-9]{1,16}` closes that off completely:
/// no `.`, `/`, `\`, or non-ASCII byte survives, so there is nothing left
/// for a path-traversal or absolute-path payload to use.
fn validate_format_token(field: &'static str, token: &str) -> Result<(), ConvertError> {
    let ok = !token.is_empty()
        && token.len() <= MAX_FORMAT_TOKEN_LEN
        && token.chars().all(|c| c.is_ascii_alphanumeric());
    if ok {
        Ok(())
    } else {
        Err(ConvertError::InvalidFormat { field })
    }
}

/// Validate `source_format` and `target_format` together, so a caller pays
/// one round trip through [`validate_format_token`] instead of ordering two
/// calls itself. Returns the first invalid field's error.
pub(super) fn validate_format_pair(
    source_format: &str,
    target_format: &str,
) -> Result<(), ConvertError> {
    validate_format_token("source_format", source_format)?;
    validate_format_token("target_format", target_format)?;
    Ok(())
}

/// Output path for one book's conversion to `target_format`:
/// `<convert_dir>/<book_id>.<target_format lowercased>`. Errors with
/// [`ConvertError::InvalidFormat`] rather than building a path from an
/// unsafe token — see [`validate_format_token`].
pub fn convert_path(book_id: i64, target_format: &str) -> Result<PathBuf, ConvertError> {
    validate_format_token("target_format", target_format)?;
    Ok(convert_dir().join(format!("{book_id}.{}", target_format.to_lowercase())))
}
