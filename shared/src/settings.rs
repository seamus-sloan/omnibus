//! User-configurable library paths and the `GET /api/library` response shape.
//!
//! These types straddle every client (mobile + web) and the server, so they
//! live here rather than next to the handler that produces them.

use serde::{Deserialize, Serialize};

/// Maximum byte length of a library path field.
pub const PATH_MAX_LEN: usize = 4096;

/// Validation failure modes for [`Settings`].
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
    /// Validate field lengths. Lengths are measured in bytes (filesystem
    /// `PATH_MAX` semantics), not Unicode scalar values.
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

/// Loose email plausibility check: exactly one `@`, non-empty local part, a
/// dotted domain, no whitespace, within [`crate::EMAIL_MAX_LEN`]. Not
/// RFC-complete — just enough to reject obvious typos before handing an
/// address to the SMTP layer. Shared by the SMTP `from` and Kindle-email
/// validators.
pub fn is_plausible_email(s: &str) -> bool {
    let s = s.trim();
    if s.is_empty() || s.len() > crate::EMAIL_MAX_LEN || s.contains(char::is_whitespace) {
        return false;
    }
    let mut parts = s.split('@');
    match (parts.next(), parts.next(), parts.next()) {
        (Some(local), Some(domain), None) => {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        }
        _ => false,
    }
}

/// Transport security for the outbound SMTP connection.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SmtpSecurity {
    /// Upgrade a plaintext connection with STARTTLS (typical on port 587).
    #[default]
    Starttls,
    /// Implicit TLS from the first byte (typical on port 465).
    Tls,
}

impl SmtpSecurity {
    /// Wire token used in the `settings` KV table and the REST/RPC payloads.
    pub fn as_str(self) -> &'static str {
        match self {
            SmtpSecurity::Starttls => "starttls",
            SmtpSecurity::Tls => "tls",
        }
    }

    /// Parse the stored token back into the enum, defaulting to STARTTLS for
    /// any unrecognized value so a hand-edited `settings` row can't panic.
    pub fn from_str_or_default(s: &str) -> Self {
        match s {
            "tls" => SmtpSecurity::Tls,
            _ => SmtpSecurity::Starttls,
        }
    }
}

/// Admin request to save (or partially update) the server-wide SMTP config.
///
/// Deliberately does not derive `Debug`: it carries a plaintext SMTP password,
/// and a stray `tracing::debug!(?req)` would write it to logs. `password` is
/// `None` to leave the stored password unchanged (so an admin can edit the
/// host without re-typing the secret); a `Some("")` clears it.
#[derive(Clone, Serialize, Deserialize)]
pub struct SmtpConfigUpdate {
    pub host: String,
    pub port: u16,
    pub username: String,
    pub from_email: String,
    #[serde(default)]
    pub security: SmtpSecurity,
    #[serde(default)]
    pub password: Option<String>,
}

/// Masked status of the server-wide SMTP config for the Settings UI. **Never
/// carries the raw password** — only a short masked preview.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct SmtpConfigStatus {
    pub configured: bool,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub from_email: Option<String>,
    pub security: SmtpSecurity,
    /// Short masked preview of the password (e.g. `pa…rd`), or `None` when unset.
    pub password_masked: Option<String>,
    /// Where the effective config comes from: `"settings"`, `"env"`, or `"none"`.
    pub source: String,
}

/// Amazon Send-to-Kindle size limits — Kindle-imposed and provider-independent.
/// We deliberately don't model per-SMTP-provider caps (e.g. Gmail's 25 MB),
/// only Amazon's own: email delivery of a personal document rejects messages
/// over 50 MB, while the web / app uploader at [`KINDLE_WEB_UPLOAD_URL`] accepts
/// up to 200 MB. An EPUB over the email cap disables the email button and points
/// the user at the uploader instead.
///
/// Expressed as **decimal** megabytes (50 * 10^6) to match Amazon's documented
/// "50 MB" / "200 MB" figures and the UI copy — not binary MiB. The decimal
/// value is also the slightly stricter cap, so we never pass a file the email
/// path would then reject.
pub const KINDLE_EMAIL_MAX_BYTES: u64 = 50_000_000;

/// Upper bound of the Send-to-Kindle web / app uploader, surfaced in the UI hint
/// so the user knows the larger path exists. Decimal MB, as with the email cap.
pub const KINDLE_WEB_MAX_BYTES: u64 = 200_000_000;

/// Amazon's Send-to-Kindle web uploader. Linked from the disabled email button
/// as the fallback for oversized files.
pub const KINDLE_WEB_UPLOAD_URL: &str = "https://www.amazon.com/sendtokindle";

/// Whether an EPUB of `size_bytes` is too large to email to Kindle (strictly
/// over [`KINDLE_EMAIL_MAX_BYTES`]). Shared so the disabled-button gate in the
/// UI and the worker-side send guard agree on one threshold. Compares the raw
/// file size against Amazon's documented 50 MB figure — base64 transfer
/// encoding inflates the on-wire message further, so a file just under the cap
/// can still bounce; those rare boundary failures fall through to the normal
/// send-error toast.
pub fn kindle_email_oversize(size_bytes: u64) -> bool {
    size_bytes > KINDLE_EMAIL_MAX_BYTES
}

/// Terminal-or-pending status of a Send-to-Kindle job, returned by the
/// `kindle/send/status` poll endpoint keyed on the `task_id` that the enqueue
/// call handed back. The send runs on the background worker; the client posts
/// once, then polls this until it flips off `Pending`. `Failed.message` is the
/// per-case reason surfaced to the button toast.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum KindleSendStatus {
    Pending,
    Sent,
    Failed { message: String },
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
