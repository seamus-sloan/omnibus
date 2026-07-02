//! Reading / listening progress wire types.
//!
//! Used by `POST /api/progress`, `GET /api/progress/{uuid}`, and
//! `POST /api/progress/sessions`. The `ProgressFormat` discriminator selects
//! which position field is meaningful so a single endpoint covers both
//! reading (EPUB CFI) and listening (audio seconds) positions.

use serde::{Deserialize, Serialize};

#[cfg(test)]
mod tests;

/// Hard cap on `SessionReport`s per batch, enforced by both the REST
/// `POST /api/progress/sessions` handler and the web
/// `rpc_record_sessions` server function. The two paths differ in how
/// they write: REST commits the whole batch inside one SQLite write
/// transaction, so an unbounded batch would hold the WAL write lock
/// for the entire loop; the RPC path calls
/// `db::progress::record_session` sequentially — one transaction per
/// report — so an unbounded batch instead lets a client drive an
/// arbitrary-length write storm. Capping the batch here bounds both
/// failure modes at the API boundary.
pub const SESSION_BATCH_CAP: usize = 500;

/// Discriminator for the format-specific payload variant in [`ProgressUpdate`]
/// / [`ProgressRecord`] / [`SessionReport`]. Serializes as a plain
/// lowercase string (`"epub"` / `"audio"`) so the wire shape stays compact.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ProgressFormat {
    Epub,
    Audio,
}

/// Progress-sync write payload. `format` discriminates which position
/// field is meaningful: `Epub` requires `epub_cfi`, `Audio` requires
/// `audio_position_seconds`. The server validates this at the handler
/// boundary via [`ProgressUpdate::validate`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProgressUpdate {
    pub book_uuid: String,
    pub format: ProgressFormat,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub epub_cfi: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio_position_seconds: Option<f64>,
}

impl ProgressUpdate {
    /// Reject empty UUIDs and missing format-specific positions. Mirrors
    /// `MetadataOverrides::validate` — handlers translate `Err(_)` into 400.
    pub fn validate(&self) -> Result<(), String> {
        if self.book_uuid.trim().is_empty() {
            return Err("book_uuid is required".into());
        }
        // Reject the non-discriminated field at the API boundary so a
        // cross-format payload (e.g. `{format:"epub", audio_position_seconds:…}`)
        // returns 400 instead of falling through to the migration 0013 CHECK
        // constraint and surfacing as a 500.
        match self.format {
            ProgressFormat::Epub => {
                if self
                    .epub_cfi
                    .as_deref()
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true)
                {
                    return Err("epub_cfi is required for format=epub".into());
                }
                if self.audio_position_seconds.is_some() {
                    return Err("audio_position_seconds must not be set for format=epub".into());
                }
            }
            ProgressFormat::Audio => {
                let Some(pos) = self.audio_position_seconds else {
                    return Err("audio_position_seconds is required for format=audio".into());
                };
                if !pos.is_finite() || pos < 0.0 {
                    return Err(
                        "audio_position_seconds must be a non-negative finite number".into(),
                    );
                }
                if self.epub_cfi.is_some() {
                    return Err("epub_cfi must not be set for format=audio".into());
                }
            }
        }
        Ok(())
    }
}

/// Server-authoritative position returned by `POST /api/progress` and
/// `GET /api/progress/{uuid}`. The non-discriminated field for the other
/// format is always `None`. `updated_at` is unix seconds (SQLite
/// `strftime('%s')`).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProgressRecord {
    pub book_uuid: String,
    pub format: ProgressFormat,
    pub epub_cfi: Option<String>,
    pub audio_position_seconds: Option<f64>,
    pub updated_at: i64,
}

/// Batched session row (reader / audio open-to-close span). Mobile
/// posts these on reconnect via `POST /api/progress/sessions`; web posts
/// best-effort on unmount. `progress_units` is seconds_read (epub) or
/// seconds_listened (audio).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SessionReport {
    pub book_uuid: String,
    pub format: ProgressFormat,
    pub started_at: i64,
    pub ended_at: i64,
    pub progress_units: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub device_id: Option<i64>,
}

impl SessionReport {
    /// Reject empty UUIDs, inverted time ranges, and negative durations
    /// before they reach the `reading_sessions` / `listening_sessions`
    /// tables — these rows feed future stats / year-in-review, so a single
    /// negative `progress_units` would skew aggregates indefinitely.
    pub fn validate(&self) -> Result<(), String> {
        if self.book_uuid.trim().is_empty() {
            return Err("book_uuid is required".into());
        }
        if self.started_at < 0 || self.ended_at < 0 {
            return Err("started_at and ended_at must be non-negative".into());
        }
        if self.ended_at < self.started_at {
            return Err("ended_at must be greater than or equal to started_at".into());
        }
        if self.progress_units < 0 {
            return Err("progress_units must be non-negative".into());
        }
        Ok(())
    }
}
