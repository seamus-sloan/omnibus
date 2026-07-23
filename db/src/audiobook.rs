//! Audiobook metadata extraction (server-only). Sibling to [`crate::ebook`]:
//! walks the configured audiobook library, reads container tags via `lofty`,
//! groups multi-file audiobooks via [`group::group_into_books`], and parses
//! them via [`parse::parse_groups`] into rows ready for
//! [`crate::sync::sync_audiobooks`].

use std::path::{Path, PathBuf};

use omnibus_shared::{Contributor, EbookMetadata};

use crate::ebook::extract_accent;

pub mod chapters;
pub mod codec;
mod cover;
mod group;
mod parse;
mod stat;

#[cfg(test)]
mod tests;

pub use chapters::{extract_chapters, RawChapter};
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

/// `book_files.format` values produced by the audiobook indexer. Used by
/// [`crate::indexer::reindex_audiobooks`] to scope the diff's "currently
/// indexed" view to audiobook rows — prevents `sync_audiobooks` from
/// classifying foreign (ebook) rows under a shared `library_path` as
/// Removed and deleting them. Uppercased to match the values written
/// in [`crate::sync::sync_audiobooks`].
pub const AUDIOBOOK_FORMATS: &[&str] = &["M4B", "M4A", "MP3"];

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
        let (h, m) = duration_to_hm(d);
        format!("Audiobook · {h}h {m:02}m")
    });
    let accent = cover
        .as_ref()
        .and_then(|(_mime, bytes)| extract_accent(bytes));

    Ok(IndexedBook {
        metadata: EbookMetadata {
            filename,
            title: Some(title),
            description,
            creators,
            accent,
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

/// Book-level metadata derived by inspecting one or more audiobook files
/// *before* they are filed into the library (the upload confirm step). For a
/// single `.m4b`/`.m4a` this is that file's own tags; for a set of `.mp3`
/// parts the title comes from the album tag (the per-file title tag is the
/// chapter name) and the author from the first non-empty artist.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct InspectedAudiobook {
    pub title: Option<String>,
    pub author: Option<String>,
    pub has_cover: bool,
    /// Combined runtime across every readable part, when tags supplied it.
    pub duration_seconds: Option<f64>,
}

/// Inspect the audiobook file(s) at `paths` and derive book-level metadata for
/// the upload confirm step, without touching the library. `paths` is one file
/// for `.m4b`/`.m4a` or the ordered `.mp3` parts of a multi-file audiobook.
///
/// Per-part tag failures are tolerated (logged, skipped) so one corrupt file
/// doesn't sink the inspect; [`AudiobookError::Unsupported`] is returned only
/// when *no* part yields readable tags.
pub fn inspect_audiobook_files(paths: &[PathBuf]) -> Result<InspectedAudiobook, AudiobookError> {
    let multi = paths.len() > 1;
    let mut album_title: Option<String> = None;
    let mut first_title: Option<String> = None;
    let mut author: Option<String> = None;
    let mut has_cover = false;
    let mut total_secs = 0.0f64;
    let mut any_readable = false;

    for (i, path) in paths.iter().enumerate() {
        // Open once and reuse the same `TaggedFile` handle for both tags and
        // cover art — lofty exposes both from one parsed handle, so a second
        // `read_from_path` per part would double the file I/O for no reason.
        match lofty::read_from_path(path) {
            Ok(tagged) => {
                any_readable = true;
                let meta = parse::metadata_from_tagged(&tagged);
                if album_title.is_none() {
                    album_title = meta.album.clone();
                }
                if i == 0 {
                    first_title = meta.title.clone();
                }
                if author.is_none() {
                    author = meta.artist.clone();
                }
                if let Some(d) = meta.duration_seconds {
                    total_secs += d;
                }
                if !has_cover && cover::cover_from_tagged(&tagged).is_some() {
                    has_cover = true;
                }
            }
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "inspect: failed to read tags");
            }
        }
    }

    if !any_readable {
        return Err(AudiobookError::Unsupported(
            "no readable audio tags in upload".to_string(),
        ));
    }

    // Multi-file: the book title lives in the album tag; single-file: the
    // file's own title tag is the book title.
    let title = if multi {
        album_title.or(first_title)
    } else {
        first_title.or(album_title)
    };

    Ok(InspectedAudiobook {
        title,
        author,
        has_cover,
        duration_seconds: (total_secs > 0.0).then_some(total_secs),
    })
}

fn filename_stem(filename: &str) -> String {
    PathBuf::from(filename)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(std::string::ToString::to_string)
        .unwrap_or_else(|| filename.to_string())
}

/// Split a duration in seconds into `(hours, minutes)` for the human-readable
/// `Audiobook · {h}h {m:02}m` description. Tag-supplied durations can be
/// `NaN`/`inf`/negative when a file is corrupt; without this guard those
/// values get cast straight to `i64::MIN`, surfacing as
/// `-9223372036854775808h` in the UI. Clamp to a sane upper bound so the
/// final cast cannot truncate.
pub(super) fn duration_to_hm(d: f64) -> (i64, i64) {
    if !d.is_finite() || d < 0.0 {
        return (0, 0);
    }
    let d = d.min(f64::from(i32::MAX));
    // The `is_finite`+`>= 0` filter and the `i32::MAX` clamp above guarantee
    // both quotients land in `[0, i32::MAX]` — far inside `i64` range, so
    // the `as i64` cast is now an in-range truncation, not the saturate-to
    // `-9223372036854775808` that this helper exists to prevent.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let hm = ((d / 3600.0) as i64, ((d % 3600.0) / 60.0) as i64);
    hm
}
