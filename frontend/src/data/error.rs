//! The transport-level [`DataError`] type shared by every data wrapper,
//! plus the flatten-to-text helper UI surfaces use to render one.

/// Errors surfaced by the feature-gated data transport.
///
/// Replaces the previous `Result<T, String>` so callers can distinguish
/// failure modes by type — most importantly `Unauthorized`, which the
/// mobile 401 handler and the web router both key on. The variants that
/// carry a foreign error type (`reqwest`, `serde_json`) are feature-gated
/// to match the optional deps that provide them: `reqwest` is mobile-only,
/// `serde_json` is web+mobile. `Unauthorized`, `Http`, and the `Other`
/// catch-all are always present so the enum's public shape is stable
/// across every build that compiles the callers.
#[derive(Debug, thiserror::Error)]
pub enum DataError {
    /// `reqwest`-level failure on the mobile transport: connect / timeout /
    /// TLS **and** response-body decode errors. The mobile calls deserialize
    /// via `response.json()`, which surfaces a malformed body as a
    /// `reqwest::Error` (`reqwest::Error::is_decode()`), not a
    /// `serde_json::Error` — so a decode failure on mobile lands here rather
    /// than in [`DataError::Decode`]. That `Decode` variant is produced only
    /// by the web/SSR path, which deserializes through `serde_json` directly.
    /// Mobile-only because `reqwest` is only linked under `feature = "mobile"`.
    #[cfg(feature = "mobile")]
    #[error("network error: {0}")]
    Network(#[from] reqwest::Error),
    /// The server responded with a non-success status (other than 401, which
    /// maps to [`DataError::Unauthorized`]). `body` carries the server's
    /// diagnostic text so callers that surface it — e.g. the register-error
    /// classifier — keep working.
    #[error("server returned {status}")]
    Http { status: u16, body: String },
    /// Response body could not be deserialized into the expected type.
    #[cfg(any(feature = "mobile", feature = "web"))]
    #[error("response deserialization failed: {0}")]
    Decode(#[from] serde_json::Error),
    /// Authentication failed (HTTP 401). Distinct variant so the 401 →
    /// clear-token → redirect-to-/login flow can pattern-match instead of
    /// re-inspecting a raw status code.
    #[error("unauthorized")]
    Unauthorized,
    /// Fast-fail result when the client already knows the server is
    /// unreachable (`offline::sync::is_offline()`) and skipped the network
    /// entirely. Unconditional (like [`DataError::Unauthorized`]) so the
    /// enum shape is stable across feature builds.
    #[error("You're offline")]
    Offline,
    /// Catch-all for transport paths that don't carry a typed source —
    /// the web server-function client (whose error is already stringified
    /// by `note_server_fn_err`), the `gloo-net` web/SSR stubs, and a couple
    /// of protocol invariants (missing JSON field, absent bearer token).
    #[error("{0}")]
    Other(String),
}

impl DataError {
    /// `true` when this represents an authentication failure. Lets callers
    /// branch on auth without depending on a specific HTTP code.
    pub fn is_unauthorized(&self) -> bool {
        matches!(self, DataError::Unauthorized)
    }
}

/// Flatten a [`DataError`] into diagnostic text for a UI surface that
/// already treats the server body as user-facing. For an HTTP failure this
/// splices the server's response body back in — `DataError`'s own `Display`
/// deliberately omits it (see the `Http` variant's doc comment) — so a
/// 409/403 renders its actual reason instead of a bare status code. Every
/// other variant falls back to its own `Display`. Not a default for every
/// caller: some contexts (e.g. offline downloads) deliberately avoid echoing
/// raw server bodies to the user.
pub fn server_error_message(err: &DataError) -> String {
    match err {
        DataError::Http { status, body } if !body.is_empty() => format!("{status}: {body}"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests;
