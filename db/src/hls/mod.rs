//! HLS transcode cache for audiobooks.
//!
//! One ffmpeg invocation per `(book_id, profile)` streams all segments
//! into `$OMNIBUS_DATA_DIR/hls/<book_id>/<profile>/` (no rename-style
//! atomicity — partials are wiped via `cleanup_segment_dir` on failure
//! so a retry starts from a clean slate). The only profile today is
//! `"audio64"` (AAC-LC 64 kbps mono, 10 s segments). Segments are served
//! directly by the axum handler via `tower-http` `ServeFile`; the
//! manifest is built per-request from the stored
//! `book_file_parts.duration_seconds` values so it is always accurate even
//! before a transcode completes.

mod fs;
mod manifest;
mod query;
mod transcode;

pub use fs::{
    cap_bytes, failed_path, ffmpeg_manifest_path, has_failed, hls_dir, is_ready, progress_path,
    read_ffmpeg_manifest, read_progress, segment_dir,
};
pub use manifest::{build_manifest, ffmpeg_progress_fraction, parse_ffmpeg_progress_us};
pub use query::{
    count_audio_files, get_chapters, get_chapters_bulk, get_parts, get_parts_bulk,
    resolve_audiobook, resolve_audiobook_file,
};
pub use transcode::{evict_if_over_cap, transcode_book, used_bytes};

/// Errors returned by the HLS DB queries. `transcode_book` uses
/// `anyhow::Result` because ffmpeg + filesystem + timeouts dominate its
/// failure space and callers just propagate.
#[derive(Debug, thiserror::Error)]
pub enum HlsError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// The only HLS audio profile shipped today. Future: add `"audio128"` for
/// music audiobooks that warrant higher fidelity.
pub const AUDIO64: &str = "audio64";

/// One part of a multi-file audiobook as read from `book_file_parts`.
/// Only the fields needed by the HLS manifest builder and transcode runner.
#[derive(Debug, Clone)]
pub struct HlsPart {
    /// Playlist ordering (from `book_file_parts.ordinal`).
    pub ordinal: i64,
    /// Library-relative path (e.g. `"Author/Book/01.mp3"`).
    pub filename: String,
    /// Duration in seconds from `book_file_parts.duration_seconds`.
    pub duration_seconds: f64,
}

/// Resolved identifiers for an audiobook uuid lookup.
pub struct ResolvedAudiobook {
    /// `books.id`
    pub book_id: i64,
    /// `book_files.id`
    pub book_file_id: i64,
    /// `scan_roots.path` — the library root on disk.
    pub library_path: String,
}

#[cfg(test)]
mod tests;
