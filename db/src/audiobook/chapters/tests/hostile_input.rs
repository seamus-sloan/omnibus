//! Hostile bytes: the MP4 box-tree and ID3v2 parsers run on arbitrary
//! user-supplied files during indexing, so these assert only that each
//! terminates gracefully — no panic, no hang — on truncation at every
//! offset, oversized size fields, count/payload mismatches and empty
//! input.

use super::super::*;
use super::{box_with, make_id3v2_chap_fixture, temp_with_bytes};

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
