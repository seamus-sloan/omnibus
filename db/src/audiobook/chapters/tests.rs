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
