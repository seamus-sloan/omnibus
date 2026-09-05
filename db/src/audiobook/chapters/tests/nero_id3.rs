//! The Nero `chpl` atom (versions 0 and 1) and ID3v2.3 / ID3v2.4 `CHAP`
//! frames, plus the empty results for a missing atom, a nonexistent file
//! and an unknown format.

use std::io::Write;
use std::path::PathBuf;

use crate::test_support::build_m4b_with_chapters;

use super::super::*;
use super::make_id3v2_chap_fixture;

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
