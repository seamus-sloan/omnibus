//! Audiobook tag extraction via `lofty`.
//!
//! Opens each file's primary tag and lifts the small set of fields the
//! basic player cares about: title, primary artist (one author — the
//! basic player has no UI for narrator vs. author roles), album, and
//! duration in seconds. Failures roll up as [`AudiobookError`] so the
//! indexer surfaces the per-file error in the same shape the EPUB path
//! uses.
//!
//! Phase B for multi-file audiobooks is handled by [`parse_groups`], which
//! reads ID3 tags for every part, sorts by (track, filename), and assembles
//! an [`IndexedAudiobook`] ready for [`crate::sync::sync_audiobooks`].

use std::path::{Path, PathBuf};

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;

use crate::ebook::extract_accent;

/// Predictable failure space for the audiobook parse + indexer dispatch.
/// `Io` covers "could not open file"; `Tag` covers any lofty decode /
/// container error; `Unsupported` is reserved for files whose extension
/// we accepted but lofty can't probe.
#[derive(Debug, thiserror::Error)]
pub enum AudiobookError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("tag decode failed: {0}")]
    Tag(#[from] lofty::error::LoftyError),
    #[error("unsupported audiobook format: {0}")]
    Unsupported(String),
}

/// Tag-only metadata view used by [`super::build_indexed_book`]. Empty
/// strings collapse to `None` so downstream defaults (filename-stem fall-
/// back for title, no-author for missing artist) kick in uniformly.
#[derive(Debug, Default, Clone)]
pub struct AudiobookMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_seconds: Option<f64>,
}

/// Open `path` with lofty and lift the basic-player tag fields. Caller
/// supplies the extension check upstream (via [`super::AUDIOBOOK_EXTENSIONS`]
/// in the scanner), so this never sees a non-audio file under normal
/// operation.
pub(super) fn extract_metadata(path: &Path) -> Result<AudiobookMetadata, AudiobookError> {
    let tagged = lofty::read_from_path(path)?;
    let duration_seconds =
        Some(tagged.properties().duration().as_secs_f64()).filter(|d| d.is_finite() && *d > 0.0);
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let mut out = AudiobookMetadata {
        duration_seconds,
        ..Default::default()
    };
    if let Some(tag) = tag {
        out.title = tag
            .title()
            .map(|c| c.trim().to_string())
            .filter(|s| !s.is_empty());
        out.artist = tag
            .artist()
            .map(|c| c.trim().to_string())
            .filter(|s| !s.is_empty());
        out.album = tag
            .album()
            .map(|c| c.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    Ok(out)
}

/// Phase B input: one entry per audiobook file the diff says needs a
/// full tag parse (the New + Changed buckets). Mirrors
/// [`crate::ebook::ParseTarget`].
#[derive(Debug, Clone)]
pub struct AudiobookParseTarget {
    pub filename: String,
    pub absolute: PathBuf,
    pub mtime_epoch: i64,
    pub size_bytes: i64,
}

/// One part of a multi-file audiobook, ready for a `book_file_parts` DB row.
#[derive(Debug, Clone)]
pub struct AudiobookPart {
    /// Playlist ordering assigned after sort-by-(track, filename).
    pub ordinal: i64,
    /// Library-relative path (e.g. `"Author/Book/01.mp3"`).
    pub filename: String,
    pub size_bytes: i64,
    pub mtime_epoch: i64,
    pub duration_seconds: f64,
}

/// One fully-parsed audiobook group, ready for [`crate::sync::sync_audiobooks`].
#[derive(Debug)]
pub struct IndexedAudiobook {
    /// Stable UUIDv5 for `(library_path, group_path)`.
    pub uuid: String,
    /// Group path (the library-relative parent dir for mp3 folders, or the
    /// file path for single-file m4b/m4a).
    pub group_path: String,
    /// Uppercased format: `"M4B"`, `"M4A"`, or `"MP3"`.
    pub format: String,
    pub title: String,
    pub creator_name: Option<String>,
    /// `(mime, bytes)` from the first part's embedded artwork. `None` when
    /// no artwork is present.
    pub cover: Option<(String, Vec<u8>)>,
    /// Cover-derived `oklch(L C H)` accent color, or `None` when the cover
    /// has no chromatic content or no cover was found.
    pub accent: Option<String>,
    pub parts: Vec<AudiobookPart>,
    /// Chapters extracted from container metadata. Empty when the format has
    /// no embedded chapter markers (the sync layer synthesizes one-per-part).
    pub chapters: Vec<super::chapters::RawChapter>,
    pub total_size_bytes: i64,
    pub max_mtime_epoch: i64,
    /// Human-readable duration, e.g. `"Audiobook · 14h 07m"`.
    pub description: Option<String>,
    /// Whole-group error (parse failure affecting every part). Rare; the
    /// normal per-part error path is to log a WARN and set
    /// `duration_seconds = 0`.
    pub error: Option<String>,
}

/// Phase B for multi-file audiobooks. Reads ID3 tags from every part in each
/// [`super::AudiobookGroup`], sorts parts by (track_number, filename), and
/// assembles one [`IndexedAudiobook`] per group.
///
/// A per-part lofty failure is logged as WARN and the part is still included
/// with `duration_seconds = 0` so one corrupt file doesn't drop the whole
/// book from the library.
pub fn parse_groups(
    groups: Vec<super::AudiobookGroup>,
    library_root: &Path,
) -> Vec<IndexedAudiobook> {
    groups
        .into_iter()
        .map(|g| parse_one_group(g, library_root))
        .collect()
}

/// Parse a single [`super::AudiobookGroup`] into an [`IndexedAudiobook`].
fn parse_one_group(group: super::AudiobookGroup, library_root: &Path) -> IndexedAudiobook {
    // Per-part: (track_number_for_sort, filename, size_bytes, mtime_epoch, duration, metadata)
    struct PartWork {
        sort_track: u32,
        filename: String,
        size_bytes: i64,
        mtime_epoch: i64,
        duration_seconds: f64,
        meta: AudiobookMetadata,
    }

    let mut parts_work: Vec<PartWork> = Vec::with_capacity(group.parts.len());
    let mut first_cover: Option<(String, Vec<u8>)> = None;
    let mut first_cover_fetched = false;

    for stat_entry in &group.parts {
        let absolute = library_root.join(&stat_entry.filename);

        // Read track number separately via lofty for sort ordering.
        let (meta_result, track_num) = match lofty::read_from_path(&absolute) {
            Ok(tagged) => {
                let dur = Some(tagged.properties().duration().as_secs_f64())
                    .filter(|d| d.is_finite() && *d > 0.0);
                let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
                let mut m = AudiobookMetadata {
                    duration_seconds: dur,
                    ..Default::default()
                };
                let track = if let Some(t) = tag {
                    m.title = t
                        .title()
                        .map(|c| c.trim().to_string())
                        .filter(|s| !s.is_empty());
                    m.artist = t
                        .artist()
                        .map(|c| c.trim().to_string())
                        .filter(|s| !s.is_empty());
                    m.album = t
                        .album()
                        .map(|c| c.trim().to_string())
                        .filter(|s| !s.is_empty());
                    t.track().unwrap_or(999_999)
                } else {
                    999_999
                };
                (Ok(m), track)
            }
            Err(e) => {
                tracing::warn!(
                    file = %absolute.display(),
                    error = %e,
                    "audiobook part: failed to read tags"
                );
                (Err(e), 999_999u32)
            }
        };

        // Fetch embedded cover from the very first readable part.
        if !first_cover_fetched {
            first_cover_fetched = true;
            match super::cover::extract_cover(&absolute) {
                Ok(Some(c)) => first_cover = Some(c),
                Ok(None) => {}
                Err(e) => {
                    tracing::warn!(
                        file = %absolute.display(),
                        error = %e,
                        "audiobook part: failed to extract cover"
                    );
                }
            }
        }

        let (meta, duration) = match meta_result {
            Ok(m) => {
                let d = m.duration_seconds.unwrap_or(0.0);
                (m, d)
            }
            Err(_) => (AudiobookMetadata::default(), 0.0),
        };

        parts_work.push(PartWork {
            sort_track: track_num,
            filename: stat_entry.filename.clone(),
            size_bytes: stat_entry.size_bytes,
            mtime_epoch: stat_entry.mtime_epoch,
            duration_seconds: duration,
            meta,
        });
    }

    // Sort by (track_number, filename) for stable playlist order.
    parts_work.sort_by(|a, b| {
        a.sort_track
            .cmp(&b.sort_track)
            .then_with(|| a.filename.cmp(&b.filename))
    });

    // Derive book-level metadata from the sorted parts. Title falls back
    // to the album tag, then to the group path's leaf — every m4b/m4a is
    // one-file-per-group post-fix, so the leaf reads as the file stem
    // (or for an mp3 folder group, the folder name).
    let title = parts_work
        .iter()
        .find_map(|p| p.meta.album.clone())
        .unwrap_or_else(|| leaf_name(&group.group_path));

    let creator_name = parts_work
        .iter()
        .find_map(|p| p.meta.artist.clone())
        .or_else(|| parent_name(&group.group_path));

    let total_secs: f64 = parts_work.iter().map(|p| p.duration_seconds).sum();
    let description = if total_secs > 0.0 {
        let h = (total_secs / 3600.0) as i64;
        let m = ((total_secs % 3600.0) / 60.0) as i64;
        Some(format!("Audiobook · {h}h {m:02}m"))
    } else {
        None
    };

    let parts: Vec<AudiobookPart> = parts_work
        .into_iter()
        .enumerate()
        .map(|(i, p)| AudiobookPart {
            ordinal: i as i64,
            filename: p.filename,
            size_bytes: p.size_bytes,
            mtime_epoch: p.mtime_epoch,
            duration_seconds: p.duration_seconds,
        })
        .collect();

    let accent = first_cover
        .as_ref()
        .and_then(|(_mime, bytes)| extract_accent(bytes));

    // Extract chapters from every part in playlist order, shifting each
    // part's file-relative times by the cumulative duration of the parts
    // before it so the result is one continuous timeline. Parts without
    // embedded markers contribute nothing; a fully empty result gets the
    // synthetic one-chapter-per-part fallback at sync time.
    let mut chapters = Vec::new();
    let mut offset_ms = 0u64;
    for part in &parts {
        let abs = library_root.join(&part.filename);
        let part_chapters = super::chapters::extract_chapters(&abs, &group.format);
        chapters.extend(offset_chapters(part_chapters, offset_ms));
        offset_ms += (part.duration_seconds * 1000.0).round().max(0.0) as u64;
    }

    IndexedAudiobook {
        uuid: group.uuid,
        group_path: group.group_path,
        format: group.format,
        title,
        creator_name,
        cover: first_cover,
        accent,
        parts,
        chapters,
        total_size_bytes: group.total_size_bytes,
        max_mtime_epoch: group.max_mtime_epoch,
        description,
        error: None,
    }
}

/// Shift a part's file-relative chapter times onto the group's
/// continuous timeline.
fn offset_chapters(
    chapters: Vec<super::chapters::RawChapter>,
    offset_ms: u64,
) -> Vec<super::chapters::RawChapter> {
    chapters
        .into_iter()
        .map(|c| super::chapters::RawChapter {
            title: c.title,
            start_ms: c.start_ms + offset_ms,
            end_ms: c.end_ms + offset_ms,
        })
        .collect()
}

/// Leaf directory name or file stem from a group path (title fallback).
fn leaf_name(group_path: &str) -> String {
    PathBuf::from(group_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| {
            PathBuf::from(group_path)
                .file_name()
                .and_then(|s| s.to_str())
                .unwrap_or(group_path)
                .to_string()
        })
}

/// Parent directory's leaf name (creator fallback for mp3 folder groups).
fn parent_name(group_path: &str) -> Option<String> {
    PathBuf::from(group_path)
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
}

/// Phase B: parse each target sequentially and emit one
/// [`super::IndexedBook`] per success. A per-file parse failure surfaces
/// as an `IndexedBook` whose metadata carries `error = Some(_)` — same
/// shape the EPUB path uses so one bad file does not hide the rest of
/// the library.
pub fn parse_audiobook_targets(targets: Vec<AudiobookParseTarget>) -> Vec<super::IndexedBook> {
    targets
        .into_iter()
        .map(|t| {
            let mut book = match super::build_indexed_book(&t.absolute, t.filename.clone()) {
                Ok(b) => b,
                Err(e) => super::IndexedBook {
                    metadata: omnibus_shared::EbookMetadata {
                        filename: t.filename,
                        error: Some(format!("could not read audiobook: {e}")),
                        ..Default::default()
                    },
                    cover: None,
                    mtime_epoch: 0,
                    size_bytes: 0,
                },
            };
            book.mtime_epoch = t.mtime_epoch;
            book.size_bytes = t.size_bytes;
            book
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::super::chapters::RawChapter;
    use super::offset_chapters;

    #[test]
    fn offset_chapters_shifts_start_and_end_by_offset() {
        let shifted = offset_chapters(
            vec![
                RawChapter {
                    title: "One".into(),
                    start_ms: 0,
                    end_ms: 1_000,
                },
                RawChapter {
                    title: "Two".into(),
                    start_ms: 1_000,
                    end_ms: 2_500,
                },
            ],
            10_000,
        );
        assert_eq!(shifted[0].start_ms, 10_000);
        assert_eq!(shifted[0].end_ms, 11_000);
        assert_eq!(shifted[1].start_ms, 11_000);
        assert_eq!(shifted[1].end_ms, 12_500);
    }
}
