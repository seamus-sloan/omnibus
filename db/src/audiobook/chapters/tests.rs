//! Unit tests for chapter extraction.

use std::io::Write;
use std::path::PathBuf;

use super::*;

/// Build a minimal MP4 file with a Nero `chpl` atom containing the given
/// chapter entries. Returns the path to a temp file.
fn make_chpl_fixture(chapters: &[(u64, &str)]) -> (tempfile::NamedTempFile, PathBuf) {
    let mut chpl_body = Vec::new();
    // version=0, flags=0x000000
    chpl_body.extend_from_slice(&[0u8; 4]);
    // chapter count (u32 BE for version 0)
    chpl_body.extend_from_slice(&(chapters.len() as u32).to_be_bytes());
    for (start_100ns, title) in chapters {
        chpl_body.extend_from_slice(&start_100ns.to_be_bytes());
        let title_bytes = title.as_bytes();
        chpl_body.push(title_bytes.len() as u8);
        chpl_body.extend_from_slice(title_bytes);
    }

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

    // Prepend a minimal ftyp box so the file looks like a real MP4
    let mut ftyp = Vec::new();
    let ftyp_size: u32 = 16; // 8 header + 4 brand + 4 minor
    ftyp.extend_from_slice(&ftyp_size.to_be_bytes());
    ftyp.extend_from_slice(b"ftyp");
    ftyp.extend_from_slice(b"M4B ");
    ftyp.extend_from_slice(&0u32.to_be_bytes());

    let mut file = tempfile::NamedTempFile::new().unwrap();
    file.write_all(&ftyp).unwrap();
    file.write_all(&moov_box).unwrap();
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
