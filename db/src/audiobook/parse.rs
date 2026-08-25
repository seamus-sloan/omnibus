//! Audiobook tag extraction via `lofty`: title, primary artist, album, and
//! duration per file. [`parse_groups`] assembles multi-file books into an
//! [`IndexedAudiobook`] for [`crate::sync::sync_audiobooks`]. Failures roll up
//! as `anyhow::Error`, matching the EPUB path's foreign-system failure shape.

use std::path::{Path, PathBuf};

use anyhow::Context;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;

use crate::ebook::extract_accent;

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
pub(super) fn extract_metadata(path: &Path) -> anyhow::Result<AudiobookMetadata> {
    let tagged = lofty::read_from_path(path)
        .with_context(|| format!("could not read audio tags from {}", path.display()))?;
    Ok(metadata_from_tagged(&tagged))
}

/// Lift the basic-player tag fields from an already-opened lofty handle.
/// Shared by [`extract_metadata`] (opens its own handle) and
/// [`super::inspect_audiobook_files`] (reuses one handle for both tags and
/// cover art, avoiding a second open+read of the same file).
pub(super) fn metadata_from_tagged(tagged: &lofty::file::TaggedFile) -> AudiobookMetadata {
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
    out
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
    /// The Phase-A diff key (F2): the group's library-relative path. Stored
    /// in `books.scan_key`; identity (`books.uuid`) is minted fresh at
    /// insert and never derived from this.
    pub scan_key: String,
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
    parse_groups_with_progress(groups, library_root, |_| {})
}

/// [`parse_groups`] variant that calls `on_group` with each group just
/// before parsing it, so the reindex pipeline can name the book currently
/// being read in the worker progress feed.
pub fn parse_groups_with_progress(
    groups: Vec<super::AudiobookGroup>,
    library_root: &Path,
    mut on_group: impl FnMut(&super::AudiobookGroup),
) -> Vec<IndexedAudiobook> {
    groups
        .into_iter()
        .map(|g| {
            on_group(&g);
            parse_one_group(g, library_root)
        })
        .collect()
}

/// Per-part working record built during the tag-extraction phase of
/// [`parse_one_group`]. Carries the fields needed to sort by
/// (track_number, filename) and to derive book-level metadata before
/// being projected into [`AudiobookPart`].
struct PartWork {
    sort_track: u32,
    filename: String,
    size_bytes: i64,
    mtime_epoch: i64,
    duration_seconds: f64,
    meta: AudiobookMetadata,
}

/// Book-level fields derived from the sorted [`PartWork`] list.
struct BookLevel {
    title: String,
    creator_name: Option<String>,
    description: Option<String>,
}

/// Parse a single [`super::AudiobookGroup`] into an [`IndexedAudiobook`].
fn parse_one_group(group: super::AudiobookGroup, library_root: &Path) -> IndexedAudiobook {
    // Single-file groups (m4b/m4a) carry the file's own relative path as
    // `group_path`; mp3 folder groups carry their parent directory. The
    // path-derived metadata fallback needs to know which shape it is
    // looking at, so the directory offsets line up either way.
    let single_file = group.parts.len() == 1 && group.parts[0].filename == group.group_path;
    let (parts_work, first_cover) = extract_tags_and_metadata(&group, library_root);
    let (book_level, parts) = build_parts_list(parts_work, &group.group_path, single_file);
    let chapters = apply_chapters(&parts, library_root, &group.format);

    let accent = first_cover
        .as_ref()
        .and_then(|(_mime, bytes)| extract_accent(bytes));

    IndexedAudiobook {
        scan_key: group.scan_key,
        group_path: group.group_path,
        format: group.format,
        title: book_level.title,
        creator_name: book_level.creator_name,
        cover: first_cover,
        accent,
        parts,
        chapters,
        total_size_bytes: group.total_size_bytes,
        max_mtime_epoch: group.max_mtime_epoch,
        description: book_level.description,
        error: None,
    }
}

/// Read lofty tags + duration for a single audiobook part at `path`. A
/// lofty failure logs a WARN and falls back to default metadata with a
/// zero duration and a large sort_track sentinel (`999_999`, sorting last),
/// so one corrupt file doesn't drop the whole group.
fn read_part_tags(path: &Path) -> (AudiobookMetadata, u32) {
    match lofty::read_from_path(path) {
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
            (m, track)
        }
        Err(e) => {
            tracing::warn!(
                file = %path.display(),
                error = %e,
                "audiobook part: failed to read tags"
            );
            (AudiobookMetadata::default(), 999_999u32)
        }
    }
}

/// Read lofty tags + duration for every part of `group` and, in the
/// same sweep, lift the first readable embedded cover.
fn extract_tags_and_metadata(
    group: &super::AudiobookGroup,
    library_root: &Path,
) -> (Vec<PartWork>, Option<(String, Vec<u8>)>) {
    let mut parts_work: Vec<PartWork> = Vec::with_capacity(group.parts.len());
    let mut first_cover: Option<(String, Vec<u8>)> = None;
    let mut first_cover_fetched = false;

    for stat_entry in &group.parts {
        let absolute = library_root.join(&stat_entry.filename);

        let (meta, track_num) = read_part_tags(&absolute);

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

        let duration = meta.duration_seconds.unwrap_or(0.0);

        parts_work.push(PartWork {
            sort_track: track_num,
            filename: stat_entry.filename.clone(),
            size_bytes: stat_entry.size_bytes,
            mtime_epoch: stat_entry.mtime_epoch,
            duration_seconds: duration,
            meta,
        });
    }

    (parts_work, first_cover)
}

/// Sort the per-part working records by (track_number, filename) for a
/// stable playlist order, derive book-level title/creator/description
/// from the sorted view, and project to the [`AudiobookPart`] list.
fn build_parts_list(
    mut parts_work: Vec<PartWork>,
    group_path: &str,
    single_file: bool,
) -> (BookLevel, Vec<AudiobookPart>) {
    parts_work.sort_by(|a, b| {
        a.sort_track
            .cmp(&b.sort_track)
            .then_with(|| a.filename.cmp(&b.filename))
    });

    // Derive book-level metadata from the sorted parts: album tag first,
    // artist tag first, then the path-derived fallback for whatever the
    // tags left empty.
    let (fallback_title, fallback_creator) = path_fallback(group_path, single_file);
    let title = parts_work
        .iter()
        .find_map(|p| p.meta.album.clone())
        .unwrap_or(fallback_title);

    let creator_name = parts_work
        .iter()
        .find_map(|p| p.meta.artist.clone())
        .or(fallback_creator);

    let total_secs: f64 = parts_work.iter().map(|p| p.duration_seconds).sum();
    let description = if total_secs > 0.0 {
        let (h, m) = super::duration_to_hm(total_secs);
        Some(format!("Audiobook · {h}h {m:02}m"))
    } else {
        None
    };

    let parts: Vec<AudiobookPart> = parts_work
        .into_iter()
        .enumerate()
        .map(|(i, p)| AudiobookPart {
            ordinal: i64::try_from(i).unwrap_or(i64::MAX),
            filename: p.filename,
            size_bytes: p.size_bytes,
            mtime_epoch: p.mtime_epoch,
            duration_seconds: p.duration_seconds,
        })
        .collect();

    (
        BookLevel {
            title,
            creator_name,
            description,
        },
        parts,
    )
}

/// Extract chapters from every part in playlist order, shifting each
/// part's file-relative times by the cumulative duration of the parts
/// before it so the result is one continuous timeline. Parts without
/// embedded markers contribute nothing; a fully empty result gets the
/// synthetic one-chapter-per-part fallback at sync time.
fn apply_chapters(
    parts: &[AudiobookPart],
    library_root: &Path,
    format: &str,
) -> Vec<super::chapters::RawChapter> {
    let mut chapters = Vec::new();
    let mut offset_ms = 0u64;
    for part in parts {
        let abs = library_root.join(&part.filename);
        let part_chapters = super::chapters::extract_chapters(&abs, format);
        chapters.extend(offset_chapters(part_chapters, offset_ms));
        // `duration_seconds` is a tag-supplied f64; guard against NaN/inf
        // and oversize values so the cumulative `offset_ms` stays
        // monotonic and bounded. Rust's float→int `as` saturates
        // (NaN→0, overflow→u64::MAX) rather than being undefined, but a
        // saturated `u64::MAX` would still wreck every subsequent chapter
        // offset — hence the explicit finite/positive/min-cap guard below.
        let ms = (part.duration_seconds * 1000.0).round();
        if ms.is_finite() && ms > 0.0 {
            let bounded = ms.min(1.0e18);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let added = bounded as u64;
            offset_ms = offset_ms.saturating_add(added);
        }
    }
    chapters
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

/// Path-derived `(title, creator)` fallback for groups whose tags carry
/// neither. An mp3 folder group's `group_path` *is* the title directory, so
/// leaf = title and parent = creator. A single-file group's `group_path` is
/// the file itself, one level deeper — under the standard
/// `<Author>/<Title>/<file>` layout the title is the parent directory and
/// the creator its grandparent (#2073). Shallower single-file layouts
/// degrade: `<Author>/<file>` takes parent as creator and the file stem
/// (minus any duplicated `<creator> - ` prefix) as title; a bare `<file>`
/// keeps the stem with no creator.
fn path_fallback(group_path: &str, single_file: bool) -> (String, Option<String>) {
    if !single_file {
        return (leaf_name(group_path), parent_name(group_path));
    }
    let path = Path::new(group_path);
    let parent = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty());
    let grandparent = path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.file_name())
        .and_then(|s| s.to_str())
        .filter(|s| !s.is_empty());
    match (parent, grandparent) {
        (Some(title_dir), Some(author_dir)) => {
            (title_dir.to_string(), Some(author_dir.to_string()))
        }
        (Some(author_dir), None) => {
            let creator = author_dir.to_string();
            let title = strip_creator_prefix(&leaf_name(group_path), &creator);
            (title, Some(creator))
        }
        _ => (leaf_name(group_path), None),
    }
}

/// Strip a leading `"<creator> - "` from a filename-derived title so
/// `Author - Title.m4b` shapes don't duplicate the author into the title.
/// ASCII-case-insensitive on the creator; a title that is *only* the prefix is
/// left untouched rather than emptied.
fn strip_creator_prefix(title: &str, creator: &str) -> String {
    title
        .get(..creator.len())
        .filter(|head| head.eq_ignore_ascii_case(creator))
        .and_then(|_| title.get(creator.len()..))
        .and_then(|rest| rest.strip_prefix(" - "))
        .filter(|rest| !rest.trim().is_empty())
        .map_or_else(|| title.to_string(), |rest| rest.trim_start().to_string())
}

/// Leaf directory name or file stem from a group path (title fallback).
fn leaf_name(group_path: &str) -> String {
    PathBuf::from(group_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .map(std::string::ToString::to_string)
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
        .map(std::string::ToString::to_string)
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
                    word_count: None,
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
    use std::io::Write;

    use super::super::chapters::tests::{make_chapter_track_fixture, TrackLayout};
    use super::super::chapters::RawChapter;
    use super::{
        apply_chapters, offset_chapters, path_fallback, strip_creator_prefix, AudiobookPart,
    };

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

    #[test]
    fn apply_chapters_lays_a_mixed_chpl_and_chapter_track_group_on_one_timeline() {
        // A group's parts are read independently — one may carry a QuickTime
        // chapter track and the next a Nero `chpl` — then each is shifted by
        // the cumulative duration of the parts before it.
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let mut file = std::fs::File::create(dir.path().join(name)).unwrap();
            file.write_all(bytes).unwrap();
        };

        let chapters: Vec<(u32, &[u8])> = vec![(1_000, b"One".as_slice()), (2_000, b"Two")];
        write(
            "01.m4b",
            &make_chapter_track_fixture(&chapters, &TrackLayout::default()),
        );
        let chpl_only = TrackLayout {
            chpl_title: Some("Three"),
            ..TrackLayout::default()
        };
        write("02.m4b", &make_chapter_track_fixture(&chapters, &chpl_only));

        let part = |ordinal: i64, filename: &str, duration_seconds: f64| AudiobookPart {
            ordinal,
            filename: filename.to_string(),
            size_bytes: 0,
            mtime_epoch: 0,
            duration_seconds,
        };
        // Part one's audio runs 3.5 s while its chapter track only covers
        // 3.0 s — the ordinary case of a muxer leaving trailing audio
        // unchaptered, and the reason the last chapter must not carry a real
        // end. With one, `sync::audiobooks` would take its `end_ms > start_ms`
        // branch and leave 3.0-3.5 s in no chapter at all.
        let parts = vec![part(0, "01.m4b", 3.5), part(1, "02.m4b", 5.0)];

        let result = apply_chapters(&parts, dir.path(), "M4B");
        assert_eq!(result.len(), 3);
        // Part one, from its chapter track, at the head of the timeline.
        assert_eq!(result[0].title, "One");
        assert_eq!((result[0].start_ms, result[0].end_ms), (0, 1_000));
        // Its last chapter keeps the sentinel, so the sync layer bridges it to
        // the next part's first chapter rather than stopping at 3.0 s.
        assert_eq!(result[1].title, "Two");
        assert_eq!((result[1].start_ms, result[1].end_ms), (1_000, 0));
        // Part two, from its `chpl`, shifted by part one's full 3.5 s of audio.
        assert_eq!(result[2].title, "Three");
        assert_eq!(result[2].start_ms, 3_500);
    }

    #[test]
    fn path_fallback_single_file_reads_title_from_parent_and_creator_from_grandparent() {
        let (title, creator) = path_fallback(
            "Logan Karlie/Dream by the Shadows/Logan Karlie - Dream by the Shadows.m4b",
            true,
        );
        assert_eq!(title, "Dream by the Shadows");
        assert_eq!(creator.as_deref(), Some("Logan Karlie"));
    }

    #[test]
    fn path_fallback_single_file_at_depth_two_takes_parent_as_creator_and_strips_prefix() {
        let (title, creator) = path_fallback("Andy Weir/Andy Weir - Project Hail Mary.m4b", true);
        assert_eq!(title, "Project Hail Mary");
        assert_eq!(creator.as_deref(), Some("Andy Weir"));
    }

    #[test]
    fn path_fallback_single_file_at_root_keeps_stem_with_no_creator() {
        let (title, creator) = path_fallback("Dracula Pt1.m4b", true);
        assert_eq!(title, "Dracula Pt1");
        assert_eq!(creator, None);
    }

    #[test]
    fn path_fallback_folder_group_keeps_leaf_title_and_parent_creator() {
        let (title, creator) = path_fallback("Bram Stoker/Dracula", false);
        assert_eq!(title, "Dracula");
        assert_eq!(creator.as_deref(), Some("Bram Stoker"));
    }

    #[test]
    fn strip_creator_prefix_is_case_insensitive_and_keeps_unrelated_titles() {
        assert_eq!(
            strip_creator_prefix("andy weir - Project Hail Mary", "Andy Weir"),
            "Project Hail Mary"
        );
        assert_eq!(
            strip_creator_prefix("Project Hail Mary", "Andy Weir"),
            "Project Hail Mary"
        );
        // A title that is only the prefix is left untouched rather than emptied.
        assert_eq!(
            strip_creator_prefix("Andy Weir - ", "Andy Weir"),
            "Andy Weir - "
        );
    }
}
