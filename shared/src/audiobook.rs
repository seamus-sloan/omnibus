//! Audiobook manifest wire types for `GET /api/audiobooks/{uuid}/manifest`.
//!
//! `Direct` is the natively-playable path (m4b/m4a/mp3/aac — Range-served per
//! part); `Hls` is the segmented-transcode fallback for codecs the browser
//! cannot decode directly (flac, ac3, eac3, …).

use serde::{Deserialize, Serialize};

/// One part of an audiobook in the direct-play manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestPart {
    pub ordinal: i64,
    pub url: String,
    pub duration_seconds: f64,
    pub mime: String,
}

/// Response payload for `GET /api/audiobooks/{uuid}/manifest`.
///
/// `direct` lists per-part URLs the client streams over HTTP Range —
/// works for any source the browser / native player decodes natively
/// (m4b, m4a, mp3, aac). `hls` falls back to the segmented transcode
/// path for codecs that need conversion (flac, ac3, eac3, …).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "mode", rename_all = "lowercase")]
pub enum AudiobookManifest {
    Direct {
        parts: Vec<ManifestPart>,
        total_duration_seconds: f64,
    },
    Hls {
        playlist_url: String,
    },
}
