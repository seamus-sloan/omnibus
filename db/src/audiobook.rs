//! Audiobook metadata extraction (server-only).
//!
//! Sibling to [`crate::ebook`]: walks the configured audiobook library,
//! reads container tags (title / artist / album / duration / cover) via
//! `lofty`, and emits rows for the DB. Multi-file audiobooks (a folder of
//! per-chapter mp3s) are grouped by [`group::group_into_books`] into a
//! single [`AudiobookGroup`] then fully parsed by
//! [`parse::parse_groups`] into [`parse::IndexedAudiobook`] rows ready for
//! [`crate::sync::sync_audiobooks`].
//!
//! Scope (F2.3 HLS player): title, primary author, album, duration in
//! seconds, embedded artwork, and per-part track ordering. Chapter atoms are
//! NOT parsed here — the chapter-list UI is deferred to a later increment.

use std::path::{Path, PathBuf};

use omnibus_shared::{Contributor, EbookMetadata};

pub mod codec;
mod cover;
mod group;
mod parse;
mod stat;

#[cfg(test)]
mod tests;

pub use codec::{classify_filenames, is_direct_playable, mime_for_filename, PlaybackMode};
pub use group::{group_into_books, AudiobookGroup};
pub use parse::{
    parse_audiobook_targets, parse_groups, AudiobookError, AudiobookParseTarget, AudiobookPart,
    IndexedAudiobook,
};
pub use stat::{stat_audiobook_library, AudiobookStatEntry, AudiobookStatScanResult};

/// Filesystem extensions the audiobook scanner picks up. Mirrors
/// [`crate::scanner::AUDIOBOOK_EXTENSIONS`] (which is used for the
/// path-count display on the settings page). Both must agree or the
/// indexer would surface files the count display ignores, or vice versa.
pub const AUDIOBOOK_EXTENSIONS: &[&str] = &["m4b", "m4a", "mp3"];

/// Re-export of [`crate::ebook::IndexedBook`] so callers using
/// `audiobook::IndexedBook` and `ebook::IndexedBook` stay in sync. The
/// sync writer is format-agnostic — it inspects the file extension via
/// [`crate::helpers::split_filename`] to derive `book_files.format`.
pub type IndexedBook = crate::ebook::IndexedBook;

/// Build the [`IndexedBook`] for a single audiobook file. Extracted as a
/// pub helper so the tests can exercise the parse path directly without
/// the full stat-and-diff machinery.
pub fn build_indexed_book(path: &Path, filename: String) -> Result<IndexedBook, AudiobookError> {
    let meta = parse::extract_metadata(path)?;
    let cover = cover::extract_cover(path)?;
    let creators = match meta.artist {
        Some(name) if !name.is_empty() => vec![Contributor {
            name,
            role: None,
            file_as: None,
            id: None,
        }],
        _ => vec![],
    };
    let title = meta
        .title
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| filename_stem(&filename));
    let description = meta.duration_seconds.map(|d| {
        let h = (d / 3600.0) as i64;
        let m = ((d % 3600.0) / 60.0) as i64;
        format!("Audiobook · {h}h {m:02}m")
    });

    Ok(IndexedBook {
        metadata: EbookMetadata {
            filename,
            title: Some(title),
            description,
            creators,
            ..Default::default()
        },
        cover,
        // Stat values get overwritten by `parse_audiobook_targets` before
        // the writer sees this struct — same pattern as
        // `ebook::parse_ebook_targets`.
        mtime_epoch: 0,
        size_bytes: 0,
    })
}

fn filename_stem(filename: &str) -> String {
    PathBuf::from(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| filename.to_string())
}
