//! User-configurable library paths and the `GET /api/library` response shape.
//!
//! These types straddle every client (mobile + web) and the server, so they
//! live here rather than next to the handler that produces them.

use serde::{Deserialize, Serialize};

/// Maximum byte length of a library path field. Paths persisted into the
/// `settings` KV are matched against on every reindex, so an unbounded
/// blob would let an authed admin push pathological strings into the row
/// or the scanner. POSIX `PATH_MAX` is typically 4 KiB; 4096 bytes covers
/// every reasonable real-world install with margin.
pub const PATH_MAX_LEN: usize = 4096;

/// Validation failure modes for [`Settings`].
///
/// Callers branch on the variant to produce the right wire response — REST
/// returns `422 Unprocessable Entity` with `e.to_string()`, the Dioxus
/// server function wraps the message in `ServerFnError::new`.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SettingsError {
    /// One of the library-path fields exceeded [`PATH_MAX_LEN`].
    #[error("{field} exceeds {PATH_MAX_LEN} bytes")]
    PathTooLong { field: &'static str },
}

/// User-configurable paths for the ebook and audiobook libraries.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Settings {
    pub ebook_library_path: Option<String>,
    pub audiobook_library_path: Option<String>,
}

impl Settings {
    /// Validate field lengths. Call at the handler boundary before
    /// persisting so an over-long path returns a typed 422 instead of
    /// landing in the `settings` KV. `None` fields (path cleared) are
    /// always permitted.
    ///
    /// Lengths are measured in bytes — paths are filesystem-level and
    /// match the kernel's `PATH_MAX` semantics rather than the Unicode
    /// scalar-value cap used by user-facing text fields.
    pub fn validate(&self) -> Result<(), SettingsError> {
        if let Some(p) = &self.ebook_library_path {
            if p.len() > PATH_MAX_LEN {
                return Err(SettingsError::PathTooLong {
                    field: "ebook_library_path",
                });
            }
        }
        if let Some(p) = &self.audiobook_library_path {
            if p.len() > PATH_MAX_LEN {
                return Err(SettingsError::PathTooLong {
                    field: "audiobook_library_path",
                });
            }
        }
        Ok(())
    }
}

/// One half of the library listing (either ebooks or audiobooks).
///
/// `counts_by_ext` is an ordered list of `(extension, count)` pairs for the
/// extensions the caller asked the scanner to track. Order matches the
/// caller-provided extension list so the UI can render a predictable summary
/// line.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibrarySection {
    pub path: Option<String>,
    pub total_files: usize,
    pub counts_by_ext: Vec<(String, usize)>,
    pub error: Option<String>,
}

/// Response payload for `GET /api/library`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct LibraryContents {
    pub ebooks: LibrarySection,
    pub audiobooks: LibrarySection,
}

#[cfg(test)]
mod tests;
