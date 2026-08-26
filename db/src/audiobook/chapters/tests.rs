//! Unit tests for chapter extraction.

use std::io::Write;
use std::path::PathBuf;

use crate::test_support::build_m4b_with_chapters;

use super::*;

/// Write a Nero-`chpl` M4B fixture to a temp file. The bytes come from
/// `test_support::build_m4b_with_chapters`; this only lands them on disk,
/// since `extract_chapters` takes a path.
fn make_chpl_fixture(chapters: &[(u64, &str)]) -> (tempfile::NamedTempFile, PathBuf) {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&build_m4b_with_chapters(chapters)).unwrap();
    file.flush().unwrap();
    let path = file.path().to_path_buf();
    (file, path)
}

#[test]
fn extract_mp4_chapters_parses_nero_chpl() {
    let chapters = vec![
        (0u64, "Introduction"),
        (3_000_000_000u64, "Chapter 1"),  // 5 min in 100ns units
        (9_000_000_000u64, "Chapter 2"),  // 15 min
        (18_000_000_000u64, "Chapter 3"), // 30 min
    ];
    let (_file, path) = make_chpl_fixture(&chapters);

    let result = extract_chapters(&path, "M4B");
    assert_eq!(result.len(), 4);
    assert_eq!(result[0].title, "Introduction");
    assert_eq!(result[0].start_ms, 0);
    assert_eq!(result[0].end_ms, 300_000); // 5 minutes in ms
    assert_eq!(result[1].title, "Chapter 1");
    assert_eq!(result[1].start_ms, 300_000);
    assert_eq!(result[1].end_ms, 900_000);
    assert_eq!(result[2].title, "Chapter 2");
    assert_eq!(result[2].start_ms, 900_000);
    assert_eq!(result[3].title, "Chapter 3");
    assert_eq!(result[3].start_ms, 1_800_000);
    assert_eq!(result[3].end_ms, 0); // Last chapter has no end
}

#[test]
fn extract_chapters_returns_empty_for_missing_chpl() {
    // File with moov but no udta/chpl
    let mut file = tempfile::NamedTempFile::new().unwrap();
    let moov_size: u32 = 8;
    file.write_all(&moov_size.to_be_bytes()).unwrap();
    file.write_all(b"moov").unwrap();
    file.flush().unwrap();

    let result = extract_chapters(file.path(), "M4B");
    assert!(result.is_empty());
}

#[test]
fn extract_chapters_returns_empty_for_nonexistent_file() {
    let result = extract_chapters(Path::new("/nonexistent/file.m4b"), "M4B");
    assert!(result.is_empty());
}

#[test]
fn extract_chapters_returns_empty_for_unknown_format() {
    let result = extract_chapters(Path::new("/some/file.flac"), "FLAC");
    assert!(result.is_empty());
}

#[test]
fn extract_mp4_chapters_handles_version_1_chpl() {
    // Version 1: 4-byte reserved field before the 1-byte count
    let mut chpl_body = Vec::new();
    chpl_body.extend_from_slice(&[1, 0, 0, 0]); // version=1, flags=0
    chpl_body.extend_from_slice(&[0u8; 4]); // reserved
    chpl_body.push(2); // 1-byte count = 2

    // Chapter 1: start at 0
    chpl_body.extend_from_slice(&0u64.to_be_bytes());
    let title = b"Prologue";
    chpl_body.push(title.len() as u8);
    chpl_body.extend_from_slice(title);

    // Chapter 2: start at 10 minutes (in 100ns units)
    let ten_min_100ns: u64 = 10 * 60 * 10_000_000;
    chpl_body.extend_from_slice(&ten_min_100ns.to_be_bytes());
    let title2 = b"Act One";
    chpl_body.push(title2.len() as u8);
    chpl_body.extend_from_slice(title2);

    let chpl_size = (8 + chpl_body.len()) as u32;
    let mut chpl_box = Vec::new();
    chpl_box.extend_from_slice(&chpl_size.to_be_bytes());
    chpl_box.extend_from_slice(b"chpl");
    chpl_box.extend_from_slice(&chpl_body);

    let udta_size = (8 + chpl_box.len()) as u32;
    let mut udta_box = Vec::new();
    udta_box.extend_from_slice(&udta_size.to_be_bytes());
    udta_box.extend_from_slice(b"udta");
    udta_box.extend_from_slice(&chpl_box);

    let moov_size = (8 + udta_box.len()) as u32;
    let mut moov_box = Vec::new();
    moov_box.extend_from_slice(&moov_size.to_be_bytes());
    moov_box.extend_from_slice(b"moov");
    moov_box.extend_from_slice(&udta_box);

    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&moov_box).unwrap();
    file.flush().unwrap();

    let result = extract_chapters(file.path(), "M4A");
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].title, "Prologue");
    assert_eq!(result[0].start_ms, 0);
    assert_eq!(result[1].title, "Act One");
    assert_eq!(result[1].start_ms, 600_000); // 10 min in ms
}

/// Build a minimal ID3v2.3 tag with one CHAP frame containing a TIT2 sub-frame.
fn make_id3v2_chap_fixture(
    major_version: u8,
    chapters: &[(u32, u32, &str)], // (start_ms, end_ms, title)
) -> Vec<u8> {
    let mut frames = Vec::new();
    for (i, (start, end, title)) in chapters.iter().enumerate() {
        // CHAP frame body: element_id(null-terminated) + start(4) + end(4) + offsets(8) + TIT2
        let element_id = format!("ch{i}\0");
        let mut tit2_body = vec![3u8]; // encoding = UTF-8
        tit2_body.extend_from_slice(title.as_bytes());
        let tit2_size = tit2_body.len() as u32;

        let mut chap_body = Vec::new();
        chap_body.extend_from_slice(element_id.as_bytes());
        chap_body.extend_from_slice(&start.to_be_bytes());
        chap_body.extend_from_slice(&end.to_be_bytes());
        chap_body.extend_from_slice(&[0xFF; 4]);
        chap_body.extend_from_slice(&[0xFF; 4]);
        // TIT2 sub-frame: 4 id + 4 size + 2 flags + body
        chap_body.extend_from_slice(b"TIT2");
        if major_version >= 4 {
            // syncsafe size
            chap_body.push(((tit2_size >> 21) & 0x7F) as u8);
            chap_body.push(((tit2_size >> 14) & 0x7F) as u8);
            chap_body.push(((tit2_size >> 7) & 0x7F) as u8);
            chap_body.push((tit2_size & 0x7F) as u8);
        } else {
            chap_body.extend_from_slice(&tit2_size.to_be_bytes());
        }
        chap_body.extend_from_slice(&[0u8; 2]); // flags
        chap_body.extend_from_slice(&tit2_body);

        let chap_size = chap_body.len() as u32;
        frames.extend_from_slice(b"CHAP");
        if major_version >= 4 {
            frames.push(((chap_size >> 21) & 0x7F) as u8);
            frames.push(((chap_size >> 14) & 0x7F) as u8);
            frames.push(((chap_size >> 7) & 0x7F) as u8);
            frames.push((chap_size & 0x7F) as u8);
        } else {
            frames.extend_from_slice(&chap_size.to_be_bytes());
        }
        frames.extend_from_slice(&[0u8; 2]); // flags
        frames.extend_from_slice(&chap_body);
    }

    let tag_size = frames.len() as u32;
    let mut data = Vec::new();
    data.extend_from_slice(b"ID3");
    data.push(major_version);
    data.push(0);
    data.push(0);
    // syncsafe tag size
    data.push(((tag_size >> 21) & 0x7F) as u8);
    data.push(((tag_size >> 14) & 0x7F) as u8);
    data.push(((tag_size >> 7) & 0x7F) as u8);
    data.push((tag_size & 0x7F) as u8);
    data.extend_from_slice(&frames);
    data
}

#[test]
fn extract_id3v2_3_chap_frames() {
    let data = make_id3v2_chap_fixture(3, &[(0, 60_000, "Intro"), (60_000, 180_000, "Chapter 1")]);
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&data).unwrap();
    file.flush().unwrap();

    let result = extract_chapters(file.path(), "MP3");
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].title, "Intro");
    assert_eq!(result[0].start_ms, 0);
    assert_eq!(result[0].end_ms, 60_000);
    assert_eq!(result[1].title, "Chapter 1");
    assert_eq!(result[1].start_ms, 60_000);
    assert_eq!(result[1].end_ms, 180_000);
}

#[test]
fn extract_id3v2_4_chap_uses_syncsafe_sizes() {
    let data = make_id3v2_chap_fixture(4, &[(0, 120_000, "Part A"), (120_000, 300_000, "Part B")]);
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&data).unwrap();
    file.flush().unwrap();

    let result = extract_chapters(file.path(), "MP3");
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].title, "Part A");
    assert_eq!(result[0].start_ms, 0);
    assert_eq!(result[1].title, "Part B");
    assert_eq!(result[1].start_ms, 120_000);
    assert_eq!(result[1].end_ms, 300_000);
}

// --- Adversarial / truncated input ---------------------------------------
//
// The MP4 box-tree and ID3v2 CHAP parsers run on arbitrary user-supplied
// files during indexing; a panic (or hang) here aborts the whole library
// scan. These tests never assert a specific parse — only that the parser
// terminates *gracefully* (returns, no panic, no hang) on hostile bytes.

/// Write raw bytes to a temp file and return it (kept alive by the caller).
fn temp_with_bytes(bytes: &[u8]) -> tempfile::NamedTempFile {
    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(bytes).unwrap();
    file.flush().unwrap();
    file
}

/// A well-formed MP4 byte stream (ftyp + moov/udta/chpl) as a flat buffer,
/// so tests can truncate it at arbitrary offsets.
fn valid_mp4_bytes() -> Vec<u8> {
    let chapters: &[(u64, &str)] = &[(0, "Intro"), (3_000_000_000, "One")];
    let mut chpl_body = Vec::new();
    chpl_body.extend_from_slice(&[0u8; 4]); // version 0 + flags
    chpl_body.extend_from_slice(&(chapters.len() as u32).to_be_bytes());
    for (start, title) in chapters {
        chpl_body.extend_from_slice(&start.to_be_bytes());
        chpl_body.push(title.len() as u8);
        chpl_body.extend_from_slice(title.as_bytes());
    }
    let chpl = box_with(b"chpl", &chpl_body);
    let udta = box_with(b"udta", &chpl);
    let moov = box_with(b"moov", &udta);

    let mut ftyp_body = Vec::new();
    ftyp_body.extend_from_slice(b"M4B ");
    ftyp_body.extend_from_slice(&0u32.to_be_bytes());
    let ftyp = box_with(b"ftyp", &ftyp_body);

    let mut out = ftyp;
    out.extend_from_slice(&moov);
    out
}

/// Wrap a body in an MP4 box header (`size(4 BE) + type(4) + body`).
fn box_with(box_type: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let size = (8 + body.len()) as u32;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(box_type);
    out.extend_from_slice(body);
    out
}

#[test]
fn extract_mp4_chapters_handles_truncation_at_every_offset_without_panicking() {
    let full = valid_mp4_bytes();
    // Cut the byte stream at every prefix length; each must return gracefully.
    for cut in 0..=full.len() {
        let file = temp_with_bytes(&full[..cut]);
        // Must not panic; result is whatever the parser salvages (often empty).
        let _ = extract_chapters(file.path(), "M4B");
    }
}

#[test]
fn extract_mp4_chapters_handles_oversized_box_length_field() {
    // A top-level box whose declared 32-bit size vastly exceeds the buffer.
    let mut data = Vec::new();
    data.extend_from_slice(&u32::MAX.to_be_bytes());
    data.extend_from_slice(b"moov");
    data.extend_from_slice(&[0u8; 4]); // a few trailing bytes, far short of the claim
    let file = temp_with_bytes(&data);
    assert!(extract_chapters(file.path(), "M4B").is_empty());
}

#[test]
fn extract_mp4_chapters_handles_extended_size_overflow_and_underrun() {
    // size == 1 selects the 64-bit extended-size path. u64::MAX would overflow
    // `pos + box_size`; a value of 8 underruns the 16-byte extended header.
    for ext in [u64::MAX, 8, 0] {
        let mut data = Vec::new();
        data.extend_from_slice(&1u32.to_be_bytes()); // size == 1 → extended
        data.extend_from_slice(b"moov");
        data.extend_from_slice(&ext.to_be_bytes());
        data.extend_from_slice(&[0u8; 8]);
        let file = temp_with_bytes(&data);
        // No panic on the checked add/sub; salvages nothing.
        assert!(extract_chapters(file.path(), "M4B").is_empty());
    }
}

#[test]
fn extract_mp4_chapters_handles_chpl_count_larger_than_payload() {
    // chpl claims 10_000 entries but carries none — reads must not run away.
    let mut chpl_body = Vec::new();
    chpl_body.extend_from_slice(&[0u8; 4]); // version 0 + flags
    chpl_body.extend_from_slice(&10_000u32.to_be_bytes());
    let chpl = box_with(b"chpl", &chpl_body);
    let udta = box_with(b"udta", &chpl);
    let moov = box_with(b"moov", &udta);
    let file = temp_with_bytes(&moov);
    assert!(extract_chapters(file.path(), "M4B").is_empty());
}

#[test]
fn extract_mp4_chapters_handles_empty_input() {
    let file = temp_with_bytes(&[]);
    assert!(extract_chapters(file.path(), "M4B").is_empty());
}

#[test]
fn extract_id3_chapters_handles_truncation_at_every_offset_without_panicking() {
    for major in [3u8, 4u8] {
        let full =
            make_id3v2_chap_fixture(major, &[(0, 60_000, "Intro"), (60_000, 180_000, "One")]);
        for cut in 0..=full.len() {
            let file = temp_with_bytes(&full[..cut]);
            let _ = extract_chapters(file.path(), "MP3");
        }
    }
}

#[test]
fn extract_id3_chapters_handles_oversized_tag_and_frame_sizes() {
    // Valid ID3 header, but the tag-size and CHAP frame-size claim far more
    // bytes than the buffer holds. Both major versions exercise the two
    // frame-size decoders (plain BE u32 for v2.3, syncsafe for v2.4).
    for major in [3u8, 4u8] {
        let mut data = Vec::new();
        data.extend_from_slice(b"ID3");
        data.push(major);
        data.push(0); // revision
        data.push(0); // flags
        data.extend_from_slice(&[0x7F, 0x7F, 0x7F, 0x7F]); // huge syncsafe tag size
        data.extend_from_slice(b"CHAP");
        data.extend_from_slice(&[0x7F, 0x7F, 0x7F, 0x7F]); // huge frame size
        data.extend_from_slice(&[0u8; 2]); // frame flags
        data.extend_from_slice(b"short"); // far fewer bytes than claimed
        let file = temp_with_bytes(&data);
        assert!(extract_chapters(file.path(), "MP3").is_empty());
    }
}

#[test]
fn extract_id3_chapters_handles_chap_body_truncated_after_element_id() {
    // A CHAP frame whose body ends right after the null-terminated element id,
    // before the four required u32 time/offset fields — must not index-panic.
    let mut chap_body = Vec::new();
    chap_body.extend_from_slice(b"ch0\0"); // element id, then nothing
    let mut frame = Vec::new();
    frame.extend_from_slice(b"CHAP");
    frame.extend_from_slice(&(chap_body.len() as u32).to_be_bytes());
    frame.extend_from_slice(&[0u8; 2]); // flags
    frame.extend_from_slice(&chap_body);

    let mut data = Vec::new();
    data.extend_from_slice(b"ID3");
    data.push(3); // v2.3 → plain BE frame sizes
    data.push(0);
    data.push(0);
    let tag_size = frame.len() as u32;
    data.push(((tag_size >> 21) & 0x7F) as u8);
    data.push(((tag_size >> 14) & 0x7F) as u8);
    data.push(((tag_size >> 7) & 0x7F) as u8);
    data.push((tag_size & 0x7F) as u8);
    data.extend_from_slice(&frame);
    let file = temp_with_bytes(&data);
    // The malformed CHAP body yields no chapter; overall result is empty.
    assert!(extract_chapters(file.path(), "MP3").is_empty());
}

#[test]
fn extract_id3_chapters_handles_empty_input() {
    let file = temp_with_bytes(&[]);
    assert!(extract_chapters(file.path(), "MP3").is_empty());
}

// --- QuickTime chapter tracks ---------------------------------------------

/// Wrap a body in a full-box header (`size + type + version + flags + body`).
fn full_box(box_type: &[u8; 4], version: u8, body: &[u8]) -> Vec<u8> {
    let mut payload = vec![version, 0, 0, 0];
    payload.extend_from_slice(body);
    box_with(box_type, &payload)
}

fn tkhd_box(track_id: u32, version: u8) -> Vec<u8> {
    let mut body = Vec::new();
    if version == 1 {
        body.extend_from_slice(&0u64.to_be_bytes()); // creation time
        body.extend_from_slice(&0u64.to_be_bytes()); // modification time
    } else {
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&0u32.to_be_bytes());
    }
    body.extend_from_slice(&track_id.to_be_bytes());
    body.extend_from_slice(&[0u8; 60]); // reserved + geometry, unread
    full_box(b"tkhd", version, &body)
}

fn mdhd_box(timescale: u32, duration: u32, version: u8) -> Vec<u8> {
    let mut body = Vec::new();
    if version == 1 {
        body.extend_from_slice(&0u64.to_be_bytes()); // creation time
        body.extend_from_slice(&0u64.to_be_bytes()); // modification time
        body.extend_from_slice(&timescale.to_be_bytes());
        body.extend_from_slice(&u64::from(duration).to_be_bytes());
    } else {
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&0u32.to_be_bytes());
        body.extend_from_slice(&timescale.to_be_bytes());
        body.extend_from_slice(&duration.to_be_bytes());
    }
    body.extend_from_slice(&[0u8; 4]); // language + quality
    full_box(b"mdhd", version, &body)
}

fn hdlr_box(handler: &[u8; 4]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&0u32.to_be_bytes()); // pre_defined
    body.extend_from_slice(handler);
    body.extend_from_slice(&[0u8; 12]); // reserved
    body.push(0); // empty name
    full_box(b"hdlr", 0, &body)
}

fn table_box(box_type: &[u8; 4], entry_count: u32, entries: &[u8]) -> Vec<u8> {
    let mut body = entry_count.to_be_bytes().to_vec();
    body.extend_from_slice(entries);
    full_box(box_type, 0, &body)
}

/// A QuickTime text sample: big-endian u16 length, then the title bytes.
fn text_sample(title: &[u8]) -> Vec<u8> {
    let mut out = (title.len() as u16).to_be_bytes().to_vec();
    out.extend_from_slice(title);
    out
}

/// How a fixture's chapter track is laid out, so one builder can cover the
/// stco/co64 and one-sample-per-chunk/all-in-one-chunk permutations.
pub(crate) struct TrackLayout {
    pub(crate) timescale: u32,
    pub(crate) samples_per_chunk: usize,
    pub(crate) co64: bool,
    pub(crate) handler: [u8; 4],
    /// Track ids the `tref`/`chap` box lists, in order.
    pub(crate) chap_refs: &'static [u32],
    /// Version byte for `tkhd`/`mdhd`; version 1 widens the creation and
    /// modification times to 64 bits and shifts every field after them.
    pub(crate) box_version: u8,
    /// When set, the fixture also carries a Nero `chpl` atom with this title.
    pub(crate) chpl_title: Option<&'static str>,
}

impl Default for TrackLayout {
    fn default() -> Self {
        Self {
            timescale: 1_000,
            samples_per_chunk: 1,
            co64: false,
            handler: *b"text",
            chap_refs: &[2],
            box_version: 0,
            chpl_title: None,
        }
    }
}

/// Build a minimal MP4 carrying a QuickTime chapter track: an audio track
/// whose `tref`/`chap` names a text track holding one sample per chapter.
/// Samples live in an `mdat` ahead of `moov` so their absolute offsets are
/// known before the sample tables are written.
pub(crate) fn make_chapter_track_fixture(
    chapters: &[(u32, &[u8])],
    layout: &TrackLayout,
) -> Vec<u8> {
    let samples: Vec<Vec<u8>> = chapters
        .iter()
        .map(|(_, title)| text_sample(title))
        .collect();

    let ftyp = {
        let mut body = b"M4B ".to_vec();
        body.extend_from_slice(&0u32.to_be_bytes());
        box_with(b"ftyp", &body)
    };
    let sample_bytes: Vec<u8> = samples.concat();
    let mdat = box_with(b"mdat", &sample_bytes);
    // ftyp, then the mdat header, then the first sample.
    let first_sample_offset = (ftyp.len() + 8) as u64;

    // Chunk offsets: the offset of each chunk's first sample.
    let mut offsets = Vec::new();
    let mut cursor = first_sample_offset;
    for (i, sample) in samples.iter().enumerate() {
        if i % layout.samples_per_chunk == 0 {
            offsets.push(cursor);
        }
        cursor += sample.len() as u64;
    }

    let stts = {
        let mut entries = Vec::new();
        for (duration, _) in chapters {
            entries.extend_from_slice(&1u32.to_be_bytes()); // sample count
            entries.extend_from_slice(&duration.to_be_bytes());
        }
        table_box(b"stts", chapters.len() as u32, &entries)
    };
    let stsc = {
        let mut entries = Vec::new();
        entries.extend_from_slice(&1u32.to_be_bytes()); // first_chunk (1-based)
        entries.extend_from_slice(&(layout.samples_per_chunk as u32).to_be_bytes());
        entries.extend_from_slice(&1u32.to_be_bytes()); // sample description index
        table_box(b"stsc", 1, &entries)
    };
    let stsz = {
        let mut body = 0u32.to_be_bytes().to_vec(); // 0 = per-sample sizes follow
        body.extend_from_slice(&(samples.len() as u32).to_be_bytes());
        for sample in &samples {
            body.extend_from_slice(&(sample.len() as u32).to_be_bytes());
        }
        full_box(b"stsz", 0, &body)
    };
    let stco = if layout.co64 {
        let entries: Vec<u8> = offsets.iter().flat_map(|o| o.to_be_bytes()).collect();
        table_box(b"co64", offsets.len() as u32, &entries)
    } else {
        let entries: Vec<u8> = offsets
            .iter()
            .flat_map(|o| (*o as u32).to_be_bytes())
            .collect();
        table_box(b"stco", offsets.len() as u32, &entries)
    };

    let stbl = box_with(b"stbl", &[stts, stsc, stsz, stco].concat());
    let minf = box_with(b"minf", &stbl);
    let total: u32 = chapters.iter().map(|(d, _)| *d).sum();
    let mdia = box_with(
        b"mdia",
        &[
            mdhd_box(layout.timescale, total, layout.box_version),
            hdlr_box(&layout.handler),
            minf,
        ]
        .concat(),
    );

    let audio_trak = {
        let ids: Vec<u8> = layout
            .chap_refs
            .iter()
            .flat_map(|id| id.to_be_bytes())
            .collect();
        let chap = box_with(b"chap", &ids);
        let tref = box_with(b"tref", &chap);
        box_with(b"trak", &[tkhd_box(1, layout.box_version), tref].concat())
    };
    let text_trak = box_with(b"trak", &[tkhd_box(2, layout.box_version), mdia].concat());

    let mut moov_children = vec![audio_trak, text_trak];
    if let Some(title) = layout.chpl_title {
        let mut body = vec![0u8; 4]; // version 0 + flags
        body.extend_from_slice(&1u32.to_be_bytes()); // one entry
        body.extend_from_slice(&0u64.to_be_bytes()); // start
        body.push(title.len() as u8);
        body.extend_from_slice(title.as_bytes());
        moov_children.push(box_with(b"udta", &box_with(b"chpl", &body)));
    }
    let moov = box_with(b"moov", &moov_children.concat());

    [ftyp, mdat, moov].concat()
}

/// The reference shape: three chapters on a millisecond timescale.
pub(crate) fn sample_chapters() -> Vec<(u32, &'static [u8])> {
    vec![
        (16_415, b"Opening Credits".as_slice()),
        (4_941_809, b"Prologue".as_slice()),
        (1_017_219, b"Chapter 1".as_slice()),
    ]
}

#[test]
fn extract_chapters_parses_quicktime_chapter_track() {
    let bytes = make_chapter_track_fixture(&sample_chapters(), &TrackLayout::default());
    let file = temp_with_bytes(&bytes);

    let result = extract_chapters(file.path(), "M4B");
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].title, "Opening Credits");
    assert_eq!(result[0].start_ms, 0);
    assert_eq!(result[0].end_ms, 16_415);
    assert_eq!(result[1].title, "Prologue");
    assert_eq!(result[1].start_ms, 16_415);
    assert_eq!(result[1].end_ms, 4_958_224);
    assert_eq!(result[2].title, "Chapter 1");
    assert_eq!(result[2].start_ms, 4_958_224);
    // The last chapter carries the `end_ms == 0` sentinel, same as `chpl`, so
    // the sync layer extends it to the book's duration rather than to the
    // chapter track's — which routinely stops short of the audio.
    assert_eq!(result[2].end_ms, 0);
}

#[test]
fn extract_chapters_scales_chapter_track_times_by_the_track_timescale() {
    // 44100 ticks per second is what a real retail M4B uses — a chapter track
    // inherits the audio sample rate rather than defaulting to milliseconds.
    let chapters: Vec<(u32, &[u8])> = vec![
        (44_100, b"One".as_slice()),   // 1s
        (66_150, b"Two".as_slice()),   // 1.5s
        (22_050, b"Three".as_slice()), // 0.5s
    ];
    let layout = TrackLayout {
        timescale: 44_100,
        ..TrackLayout::default()
    };
    let file = temp_with_bytes(&make_chapter_track_fixture(&chapters, &layout));

    let result = extract_chapters(file.path(), "M4B");
    assert_eq!(result.len(), 3);
    assert_eq!((result[0].start_ms, result[0].end_ms), (0, 1_000));
    assert_eq!((result[1].start_ms, result[1].end_ms), (1_000, 2_500));
    assert_eq!((result[2].start_ms, result[2].end_ms), (2_500, 0));
}

#[test]
fn extract_chapters_parses_chapter_track_with_co64_offsets() {
    let layout = TrackLayout {
        co64: true,
        ..TrackLayout::default()
    };
    let file = temp_with_bytes(&make_chapter_track_fixture(&sample_chapters(), &layout));

    let result = extract_chapters(file.path(), "M4B");
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].title, "Opening Credits");
    assert_eq!(result[2].title, "Chapter 1");
}

#[test]
fn extract_chapters_parses_chapter_track_with_many_samples_per_chunk() {
    // One chunk holding all three samples exercises the `stsc` run walk,
    // where sample offsets accumulate within a chunk rather than coming
    // straight from `stco`.
    let layout = TrackLayout {
        samples_per_chunk: 3,
        ..TrackLayout::default()
    };
    let file = temp_with_bytes(&make_chapter_track_fixture(&sample_chapters(), &layout));

    let result = extract_chapters(file.path(), "M4B");
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].title, "Opening Credits");
    assert_eq!(result[1].title, "Prologue");
    assert_eq!(result[2].title, "Chapter 1");
}

#[test]
fn extract_chapters_decodes_utf16_chapter_titles() {
    // A BOM selects UTF-16; QuickTime text samples use it for non-ASCII.
    let mut be = vec![0xFE, 0xFF];
    for unit in "Café".encode_utf16() {
        be.extend_from_slice(&unit.to_be_bytes());
    }
    let mut le = vec![0xFF, 0xFE];
    for unit in "Naïve".encode_utf16() {
        le.extend_from_slice(&unit.to_le_bytes());
    }
    let chapters: Vec<(u32, &[u8])> = vec![(1_000, be.as_slice()), (1_000, le.as_slice())];
    let file = temp_with_bytes(&make_chapter_track_fixture(
        &chapters,
        &TrackLayout::default(),
    ));

    let result = extract_chapters(file.path(), "M4B");
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].title, "Café");
    assert_eq!(result[1].title, "Naïve");
}

#[test]
fn extract_chapters_prefers_nero_chpl_over_a_chapter_track() {
    // A file carrying both must keep the established `chpl` reading, so the
    // new fallback can't change what already-indexed libraries report.
    let layout = TrackLayout {
        chpl_title: Some("From chpl"),
        ..TrackLayout::default()
    };
    let file = temp_with_bytes(&make_chapter_track_fixture(&sample_chapters(), &layout));

    let result = extract_chapters(file.path(), "M4B");
    assert_eq!(result.len(), 1);
    assert_eq!(result[0].title, "From chpl");
}

#[test]
fn extract_chapters_returns_empty_when_chapter_track_reference_is_dangling() {
    // `tref`/`chap` names a track the file doesn't contain.
    let layout = TrackLayout {
        chap_refs: &[99],
        ..TrackLayout::default()
    };
    let file = temp_with_bytes(&make_chapter_track_fixture(&sample_chapters(), &layout));
    assert!(extract_chapters(file.path(), "M4B").is_empty());
}

#[test]
fn extract_chapters_returns_empty_when_referenced_track_is_not_text() {
    // A `chap` pointing at an audio track must not have us decoding audio
    // samples as titles.
    let layout = TrackLayout {
        handler: *b"soun",
        ..TrackLayout::default()
    };
    let file = temp_with_bytes(&make_chapter_track_fixture(&sample_chapters(), &layout));
    assert!(extract_chapters(file.path(), "M4B").is_empty());
}

#[test]
fn extract_chapters_returns_empty_when_chapter_track_has_no_tref() {
    // Only a text track, with nothing referencing it — not chapters.
    let mut bytes = make_chapter_track_fixture(&sample_chapters(), &TrackLayout::default());
    // Blank the `chap` reference type so the tref no longer names chapters.
    let pos = bytes
        .windows(4)
        .position(|w| w == b"chap")
        .expect("fixture has a chap box");
    bytes[pos..pos + 4].copy_from_slice(b"xxxx");
    let file = temp_with_bytes(&bytes);
    assert!(extract_chapters(file.path(), "M4B").is_empty());
}

#[test]
fn extract_chapters_handles_chapter_track_truncation_at_every_offset_without_panicking() {
    let full = make_chapter_track_fixture(&sample_chapters(), &TrackLayout::default());
    for cut in 0..=full.len() {
        let file = temp_with_bytes(&full[..cut]);
        // Must not panic; result is whatever the parser salvages.
        let _ = extract_chapters(file.path(), "M4B");
    }
}

#[test]
fn extract_chapters_handles_chapter_track_sample_counts_larger_than_their_tables() {
    // Inflate each sample-table entry count far past the bytes present. Every
    // one must be rejected on the box-size check rather than sized into a
    // multi-gigabyte allocation.
    for table in [b"stts".as_slice(), b"stsc", b"stsz", b"stco"] {
        let mut bytes = make_chapter_track_fixture(&sample_chapters(), &TrackLayout::default());
        let pos = bytes
            .windows(4)
            .position(|w| w == table)
            .expect("fixture has the table");
        // `stsz` keeps its count in the second word; the rest in the first.
        let count_at = if table == b"stsz".as_slice() {
            pos + 12
        } else {
            pos + 8
        };
        bytes[count_at..count_at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
        let file = temp_with_bytes(&bytes);
        assert!(
            extract_chapters(file.path(), "M4B").is_empty(),
            "{} with an oversized count should yield nothing",
            String::from_utf8_lossy(table)
        );
    }
}

#[test]
fn extract_chapters_returns_empty_when_chapter_track_timescale_is_zero() {
    // A zero timescale would divide by zero converting ticks to milliseconds.
    let layout = TrackLayout {
        timescale: 0,
        ..TrackLayout::default()
    };
    let file = temp_with_bytes(&make_chapter_track_fixture(&sample_chapters(), &layout));
    assert!(extract_chapters(file.path(), "M4B").is_empty());
}

#[test]
fn extract_chapters_handles_chapter_track_offsets_past_end_of_file() {
    // Chunk offsets pointing beyond EOF: titles come back empty, but the
    // chapter timeline still parses and nothing panics.
    let mut bytes = make_chapter_track_fixture(&sample_chapters(), &TrackLayout::default());
    let pos = bytes
        .windows(4)
        .position(|w| w == b"stco")
        .expect("fixture has stco");
    let entries_at = pos + 12;
    for i in 0..3 {
        let at = entries_at + i * 4;
        bytes[at..at + 4].copy_from_slice(&u32::MAX.to_be_bytes());
    }
    let file = temp_with_bytes(&bytes);
    let result = extract_chapters(file.path(), "M4B");
    // Assert the count first: `all` over an empty vec is vacuously true, so
    // without this the test would still pass if extraction returned nothing.
    assert_eq!(result.len(), 3);
    assert!(result.iter().all(|c| c.title.is_empty()));
    assert_eq!(result[0].start_ms, 0);
    assert_eq!(result[1].start_ms, 16_415);
}

#[test]
fn extract_chapters_reads_version_1_tkhd_and_mdhd_field_offsets() {
    // Version 1 widens the creation and modification times to 64 bits, so the
    // track id and timescale sit 8 bytes later than in version 0. Getting
    // either offset wrong yields a track id that matches no trak, which would
    // silently drop the book back to synthetic chapters.
    let chapters: Vec<(u32, &[u8])> = vec![(44_100, b"One".as_slice()), (88_200, b"Two")];
    let layout = TrackLayout {
        timescale: 44_100,
        box_version: 1,
        ..TrackLayout::default()
    };
    let file = temp_with_bytes(&make_chapter_track_fixture(&chapters, &layout));

    let result = extract_chapters(file.path(), "M4B");
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].title, "One");
    // Correct only if the timescale was read from offset 20, not 12.
    assert_eq!((result[0].start_ms, result[0].end_ms), (0, 1_000));
    assert_eq!(result[1].title, "Two");
    assert_eq!(result[1].start_ms, 1_000);
}

#[test]
fn extract_chapters_falls_past_a_dangling_reference_to_a_readable_chapter_track() {
    // A `chap` box lists ids in order; the first here names no track. The
    // walk must keep going rather than treating one bad id as the answer.
    let layout = TrackLayout {
        chap_refs: &[99, 2],
        ..TrackLayout::default()
    };
    let file = temp_with_bytes(&make_chapter_track_fixture(&sample_chapters(), &layout));

    let result = extract_chapters(file.path(), "M4B");
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].title, "Opening Credits");
    assert_eq!(result[2].title, "Chapter 1");
}

#[test]
fn extract_chapters_trims_nul_terminated_chapter_titles() {
    // Some muxers NUL-terminate inside the declared u16 length. The
    // terminator must not reach `file_chapters` and render in the UI.
    let mut utf16 = vec![0xFE, 0xFF];
    for unit in "Prologue".encode_utf16() {
        utf16.extend_from_slice(&unit.to_be_bytes());
    }
    utf16.extend_from_slice(&[0x00, 0x00]); // UTF-16 NUL
    let chapters: Vec<(u32, &[u8])> = vec![
        (1_000, b"Opening Credits\0".as_slice()),
        (1_000, utf16.as_slice()),
    ];
    let file = temp_with_bytes(&make_chapter_track_fixture(
        &chapters,
        &TrackLayout::default(),
    ));

    let result = extract_chapters(file.path(), "M4B");
    assert_eq!(result.len(), 2);
    assert_eq!(result[0].title, "Opening Credits");
    assert_eq!(result[1].title, "Prologue");
}

#[test]
fn extract_chapters_truncates_a_chapter_track_with_more_samples_than_the_cap() {
    // Over the cap the walk truncates rather than discarding everything: a
    // partial timeline still beats falling back to synthetic chapters.
    let stts_entry = {
        let mut entries = Vec::new();
        entries.extend_from_slice(&u32::MAX.to_be_bytes()); // sample count
        entries.extend_from_slice(&1_000u32.to_be_bytes()); // delta
        entries
    };
    let mut bytes = make_chapter_track_fixture(&sample_chapters(), &TrackLayout::default());
    let pos = bytes
        .windows(4)
        .position(|w| w == b"stts")
        .expect("fixture has stts");
    // Collapse the three run-length entries into one claiming u32::MAX
    // samples, keeping the box size intact by zero-filling the remainder.
    bytes[pos + 8..pos + 12].copy_from_slice(&1u32.to_be_bytes()); // entry count
    bytes[pos + 12..pos + 20].copy_from_slice(&stts_entry);
    for b in &mut bytes[pos + 20..pos + 36] {
        *b = 0;
    }
    let file = temp_with_bytes(&bytes);

    let result = extract_chapters(file.path(), "M4B");
    // `stts` alone would yield MAX_CHAPTERS starts; the sample tables cap it
    // at the three samples that actually exist.
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].title, "Opening Credits");
}
