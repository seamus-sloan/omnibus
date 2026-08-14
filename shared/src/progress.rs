//! Reading / listening progress wire types.
//!
//! Used by `POST /api/progress`, `GET /api/progress/{uuid}`, and
//! `POST /api/progress/sessions`. The `ProgressFormat` discriminator selects
//! which position field is meaningful so a single endpoint covers both
//! reading (EPUB CFI) and listening (audio seconds) positions.

use serde::{Deserialize, Serialize};

use crate::highlight::CreateHighlight;
use crate::EbookMetadata;

#[cfg(test)]
mod tests;

/// Maximum number of `SessionReport`s accepted per session-batch upload.
///
/// Enforced at both API boundaries — the mobile REST route
/// `POST /api/progress/sessions` and the web RPC `rpc_record_sessions` —
/// to bound per-request DB work and SQLite write-lock hold time. Not
/// runtime-configurable; change here to move the cap.
pub const SESSION_BATCH_CAP: usize = 500;

/// Maximum length (in chars) of an `epub_cfi` position string. Defined in
/// terms of `CreateHighlight::EPUB_CFI_RANGE_MAX_LEN` — both hold the same
/// kind of value — so the two ceilings can't drift apart.
pub const EPUB_CFI_MAX_LEN: usize = CreateHighlight::EPUB_CFI_RANGE_MAX_LEN;

/// Position anchor for a comic (CBZ) page, stored in the `epub_cfi` slot of
/// an `Epub`-format progress row. Comics reuse the existing ebook progress
/// record — the row CHECK admits any text position plus a percent — so
/// Continue Reading and cross-device sync work unchanged, but a page index
/// is not a CFI: the `comic-page:` prefix makes the value self-describing,
/// and every client (web pager, iOS pager) round-trips it through these two
/// helpers rather than inventing its own encoding. The percent alongside it
/// is the cross-surface half, same as a Kobo's percent-only write.
pub fn comic_page_anchor(page: usize) -> String {
    format!("comic-page:{page}")
}

/// Parse a [`comic_page_anchor`] back to its 0-based page index. `None` for
/// anything else — including a real EPUB CFI — so callers can fall back to
/// `progress_percent` without misreading a foreign position.
pub fn parse_comic_page_anchor(anchor: &str) -> Option<usize> {
    anchor.strip_prefix("comic-page:")?.parse().ok()
}

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
    /// Whole-book percent, 0..=100. The cross-surface half of a position:
    /// unlike a CFI or a `KoboSpan` it means the same thing everywhere. A
    /// Kobo reports one and no CFI, so for `Epub` this satisfies the
    /// "some position" requirement on its own.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress_percent: Option<i64>,
    /// A Kobo's `CurrentBookmark.Location` object, serialized to a JSON
    /// string and stored verbatim. Echoed back to the device for exact
    /// resume; the server may additionally translate it to/from a CFI
    /// (`db::kobo_position`), but no client ever renders it, and a string
    /// keeps `shared` free of a `serde_json` runtime dependency.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kobo_location: Option<String>,
    /// The `book_files` row the position was taken in, for books carrying
    /// more than one file of the format (two narrations of one audiobook).
    /// `None` from a client that doesn't track it — the server then resolves
    /// the same default file the manifest would have served.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub book_file_id: Option<i64>,
    /// Unix seconds when the client observed this position — used to
    /// resolve most-recent-wins by **event** time rather than server
    /// receipt time (issue #1362). `#[serde(default)]` so an older client
    /// that never sends this field still parses; `upsert_progress` treats a
    /// missing value as "use server now", preserving prior last-write-wins
    /// behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_updated_at: Option<i64>,
}

impl ProgressUpdate {
    /// Reject empty UUIDs, missing format-specific positions, and a
    /// negative `client_updated_at`. Mirrors `MetadataOverrides::validate`
    /// — handlers translate `Err(_)` into 400.
    pub fn validate(&self) -> Result<(), String> {
        if self.book_uuid.trim().is_empty() {
            return Err("book_uuid is required".into());
        }
        if self.client_updated_at.is_some_and(|ts| ts < 0) {
            return Err("client_updated_at must be non-negative".into());
        }
        // Checked before the format split so an out-of-range percent is a 400
        // rather than falling through to the row CHECK as a 500.
        if self
            .progress_percent
            .is_some_and(|p| !(0..=100).contains(&p))
        {
            return Err("progress_percent must be between 0 and 100".into());
        }
        if self.book_file_id.is_some_and(|id| id <= 0) {
            return Err("book_file_id must be positive".into());
        }
        // Reject the non-discriminated field at the API boundary so a
        // cross-format payload (e.g. `{format:"epub", audio_position_seconds:…}`)
        // returns 400 instead of falling through to the migration 0013 CHECK
        // constraint and surfacing as a 500.
        match self.format {
            ProgressFormat::Epub => {
                // A present-but-blank CFI is rejected outright rather than
                // ignored: the store binds blanks as NULL, so letting one
                // through would silently clear a stored anchor instead of
                // recording the position the client thought it sent.
                if self
                    .epub_cfi
                    .as_deref()
                    .is_some_and(|s| s.trim().is_empty())
                {
                    return Err("epub_cfi must not be blank".into());
                }
                // A CFI *or* a percent is enough. A Kobo has no CFI to give
                // (its location is a `KoboSpan`, not a CFI), so requiring one
                // would lock the device out of the progress store entirely.
                if self.epub_cfi.is_none() && self.progress_percent.is_none() {
                    return Err("format=epub requires epub_cfi or progress_percent".into());
                }
                if let Some(cfi) = &self.epub_cfi {
                    if cfi.chars().count() > EPUB_CFI_MAX_LEN {
                        return Err(format!("epub_cfi exceeds {EPUB_CFI_MAX_LEN} characters"));
                    }
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
                if self.progress_percent.is_some() {
                    return Err("progress_percent must not be set for format=audio".into());
                }
                if self.kobo_location.is_some() {
                    return Err("kobo_location must not be set for format=audio".into());
                }
            }
        }
        Ok(())
    }
}

/// Server-authoritative position returned by `POST /api/progress` and
/// `GET /api/progress/{uuid}`. The non-discriminated field for the other
/// format is always `None`. `updated_at` is unix seconds (SQLite
/// `strftime('%s')`) — server receipt time. `client_updated_at` is the
/// event time the most-recent-wins ordering actually resolves on (clamped
/// to server now for a client with a fast clock; defaulted to server now
/// when the write carried none).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ProgressRecord {
    pub book_uuid: String,
    pub format: ProgressFormat,
    pub epub_cfi: Option<String>,
    pub audio_position_seconds: Option<f64>,
    /// Whole-book percent, 0..=100. `#[serde(default)]` — unlike its siblings
    /// here — so a payload from a server predating this field still decodes.
    #[serde(default)]
    pub progress_percent: Option<i64>,
    /// Opaque Kobo `CurrentBookmark.Location` JSON; see [`ProgressUpdate::kobo_location`].
    #[serde(default)]
    pub kobo_location: Option<String>,
    /// The `book_files` row the position was taken in; see
    /// [`ProgressUpdate::book_file_id`].
    #[serde(default)]
    pub book_file_id: Option<i64>,
    pub updated_at: i64,
    pub client_updated_at: i64,
}

/// "Pick up where you left off" entry returned by `GET /api/progress/recent` and `rpc_recent_progress`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ResumePoint {
    pub record: ProgressRecord,
    pub book: EbookMetadata,
    /// Whole-book audio duration (sum of parts). `None` for epub rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_duration_seconds: Option<f64>,
    /// 1-based chapter number at the saved position. `None` for epub rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter_number: Option<i64>,
    /// Total chapter count, for a "Ch. 3 of 12" readout. `None` for epub rows.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub chapter_count: Option<i64>,
    /// The user's saved playback rate for this book, so resume surfaces can
    /// rate-adjust their "left" readouts. `None` for epub rows and when no
    /// preference has been saved (treat as 1x).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub playback_rate: Option<f64>,
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
    /// Handle minted by the device that recorded the session, making a
    /// replay idempotent (migration 0052). A queued report is retried
    /// whenever the reply was lost rather than the request, and without a
    /// handle each retry appended a second row. `None` for web clients,
    /// which post once and never retry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_id: Option<String>,
}

impl SessionReport {
    /// Maximum length (in chars) of a client-minted session id. Mirror of
    /// [`CreateHighlight::CLIENT_ID_MAX_LEN`].
    pub const CLIENT_ID_MAX_LEN: usize = CreateHighlight::CLIENT_ID_MAX_LEN;

    /// Reject empty UUIDs, inverted time ranges, negative durations, and a
    /// malformed `client_id` before they reach the `reading_sessions` /
    /// `listening_sessions` tables — these rows feed future stats /
    /// year-in-review, so a single negative `progress_units` would skew
    /// aggregates indefinitely.
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
        // Same ceiling as the annotation handles: this one indexes a batched
        // route, so an unbounded string would go straight into a unique
        // index once per report in the batch.
        if let Some(ref client_id) = self.client_id {
            if client_id.trim().is_empty() {
                return Err("client_id must not be blank".into());
            }
            if client_id.chars().count() > Self::CLIENT_ID_MAX_LEN {
                return Err(format!(
                    "client_id exceeds {} characters",
                    Self::CLIENT_ID_MAX_LEN
                ));
            }
        }
        Ok(())
    }
}
