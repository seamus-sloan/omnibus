//! Audiobook manifest wire types for `GET /api/audiobooks/{uuid}/manifest`.
//!
//! `Direct` is the natively-playable path (m4b/m4a/mp3/aac — Range-served per
//! part); `Hls` is the segmented-transcode fallback for codecs the browser
//! cannot decode directly (flac, ac3, eac3, …).

use serde::{Deserialize, Serialize};

/// Slowest playback rate accepted by every audiobook client and transport.
pub const MIN_AUDIOBOOK_PLAYBACK_RATE: f64 = 0.5;
/// Fastest playback rate accepted by every audiobook client and transport.
pub const MAX_AUDIOBOOK_PLAYBACK_RATE: f64 = 3.0;

/// Request payload for changing one audiobook's playback rate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudiobookPlaybackRateUpdate {
    pub playback_rate: f64,
}

impl AudiobookPlaybackRateUpdate {
    /// Reject non-finite rates and values outside the player-supported range.
    pub fn validate(&self) -> Result<(), String> {
        if !self.playback_rate.is_finite()
            || !(MIN_AUDIOBOOK_PLAYBACK_RATE..=MAX_AUDIOBOOK_PLAYBACK_RATE)
                .contains(&self.playback_rate)
        {
            return Err(format!(
                "playback_rate must be between {MIN_AUDIOBOOK_PLAYBACK_RATE} and {MAX_AUDIOBOOK_PLAYBACK_RATE}"
            ));
        }
        Ok(())
    }
}

/// Server-authoritative playback-rate preference for one audiobook.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AudiobookPlaybackRateRecord {
    pub book_uuid: String,
    pub playback_rate: f64,
    pub updated_at: i64,
}

/// One part of an audiobook in the direct-play manifest.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ManifestPart {
    pub ordinal: i64,
    pub url: String,
    pub duration_seconds: f64,
    pub mime: String,
}

/// Chapter marker within an audiobook's timeline.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ChapterInfo {
    pub ordinal: i64,
    pub title: String,
    pub start_seconds: f64,
    pub duration_seconds: f64,
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
        #[serde(default)]
        chapters: Vec<ChapterInfo>,
    },
    Hls {
        playlist_url: String,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn playback_rate_update_accepts_supported_bounds() {
        for playback_rate in [0.5, 3.0] {
            let update = AudiobookPlaybackRateUpdate { playback_rate };
            assert!(update.validate().is_ok());
        }
    }

    #[test]
    fn playback_rate_update_rejects_rates_outside_supported_bounds() {
        for playback_rate in [0.49, 3.01] {
            let update = AudiobookPlaybackRateUpdate { playback_rate };
            assert!(update.validate().is_err());
        }
    }

    #[test]
    fn playback_rate_update_rejects_non_finite_rates() {
        for playback_rate in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
            let update = AudiobookPlaybackRateUpdate { playback_rate };
            assert!(update.validate().is_err());
        }
    }

    #[test]
    fn playback_rate_record_round_trips_over_json() {
        let record = AudiobookPlaybackRateRecord {
            book_uuid: "book-a".into(),
            playback_rate: 1.5,
            updated_at: 123,
        };
        let json = serde_json::to_string(&record).unwrap();
        assert_eq!(
            serde_json::from_str::<AudiobookPlaybackRateRecord>(&json).unwrap(),
            record
        );
    }
}
