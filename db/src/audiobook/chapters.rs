//! Chapter extraction from audiobook containers.
//!
//! M4B/M4A files store chapters as Nero `chpl` atoms (`moov/udta/chpl`) or
//! QuickTime chapter tracks. MP3 files may embed ID3v2 CHAP/CTOC frames.
//! This module extracts both via a unified [`RawChapter`] output that the
//! sync layer converts to absolute-timeline `file_chapters` rows.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// A chapter extracted from an audiobook container, prior to absolute-time
/// normalization. Times are in milliseconds relative to the file.
#[derive(Debug, Clone, PartialEq)]
pub struct RawChapter {
    pub title: String,
    pub start_ms: u64,
    pub end_ms: u64,
}

/// Extract chapters from an audiobook file. Returns an empty vec (not an
/// error) when no chapters are found or the file cannot be parsed — chapter
/// absence is non-fatal and triggers the synthetic-fallback path.
pub fn extract_chapters(path: &Path, format: &str) -> Vec<RawChapter> {
    match format.to_uppercase().as_str() {
        "M4B" | "M4A" => extract_mp4_chapters(path).unwrap_or_default(),
        "MP3" => extract_id3_chapters(path).unwrap_or_default(),
        _ => Vec::new(),
    }
}

/// Walk the MP4 box tree to find `moov/udta/chpl` (Nero chapters).
fn extract_mp4_chapters(path: &Path) -> Option<Vec<RawChapter>> {
    let mut file = std::fs::File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    let moov = find_box(&mut file, file_len, b"moov")?;
    let udta = find_child_box(&mut file, moov.data_offset, moov.data_size, b"udta")?;
    let chpl = find_child_box(&mut file, udta.data_offset, udta.data_size, b"chpl")?;
    parse_chpl_payload(&mut file, chpl.data_offset, chpl.data_size)
}

struct BoxInfo {
    data_offset: u64,
    data_size: u64,
}

/// Find a box at the current level starting from offset 0.
fn find_box(file: &mut std::fs::File, file_len: u64, target: &[u8; 4]) -> Option<BoxInfo> {
    find_child_box(file, 0, file_len, target)
}

/// Find a child box within a parent box's data region.
fn find_child_box(
    file: &mut std::fs::File,
    parent_offset: u64,
    parent_size: u64,
    target: &[u8; 4],
) -> Option<BoxInfo> {
    let end = parent_offset + parent_size;
    let mut pos = parent_offset;

    while pos + 8 <= end {
        file.seek(SeekFrom::Start(pos)).ok()?;
        let mut header = [0u8; 8];
        file.read_exact(&mut header).ok()?;

        let size = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as u64;
        let box_type = &header[4..8];

        let (box_size, data_offset) = if size == 1 {
            // 64-bit extended size
            let mut ext = [0u8; 8];
            file.read_exact(&mut ext).ok()?;
            let ext_size = u64::from_be_bytes(ext);
            (ext_size, pos + 16)
        } else if size == 0 {
            // Box extends to end of file
            (end - pos, pos + 8)
        } else {
            (size, pos + 8)
        };

        if box_size < 8 {
            return None;
        }

        if box_type == target {
            let data_size = box_size - (data_offset - pos);
            return Some(BoxInfo {
                data_offset,
                data_size,
            });
        }

        pos += box_size;
    }
    None
}

/// Parse the Nero `chpl` atom payload into chapter entries.
///
/// Format: version (1 byte) + flags (3 bytes) + entry count (4 bytes for
/// v0, 1 byte for v1) + N entries of (start_100ns: u64, title_len: u8,
/// title: [u8; title_len]).
fn parse_chpl_payload(file: &mut std::fs::File, offset: u64, size: u64) -> Option<Vec<RawChapter>> {
    if size < 5 {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut ver_flags = [0u8; 4];
    file.read_exact(&mut ver_flags).ok()?;
    let version = ver_flags[0];

    let count = if version >= 1 {
        let mut buf = [0u8; 1];
        file.read_exact(&mut buf).ok()?;
        buf[0] as usize
    } else {
        let mut buf = [0u8; 4];
        file.read_exact(&mut buf).ok()?;
        u32::from_be_bytes(buf) as usize
    };

    if count == 0 || count > 10_000 {
        return None;
    }

    let mut chapters = Vec::with_capacity(count);
    for _ in 0..count {
        let mut ts_buf = [0u8; 8];
        file.read_exact(&mut ts_buf).ok()?;
        let start_100ns = u64::from_be_bytes(ts_buf);
        let start_ms = start_100ns / 10_000;

        let mut len_buf = [0u8; 1];
        file.read_exact(&mut len_buf).ok()?;
        let title_len = len_buf[0] as usize;

        let mut title_buf = vec![0u8; title_len];
        file.read_exact(&mut title_buf).ok()?;
        let title = String::from_utf8_lossy(&title_buf).to_string();

        chapters.push(RawChapter {
            title,
            start_ms,
            end_ms: 0,
        });
    }

    // Fill end_ms from next chapter's start_ms
    for i in 0..chapters.len().saturating_sub(1) {
        chapters[i].end_ms = chapters[i + 1].start_ms;
    }

    Some(chapters)
}

/// Extract ID3v2 CHAP frames from an MP3 file.
///
/// MP3 audiobooks are almost always folder-of-files where each file IS a
/// chapter — the synthetic fallback handles that perfectly. Embedded CHAP
/// frames in a single-file MP3 are extremely rare for audiobooks, so this
/// path reads the raw bytes directly rather than fighting lofty's unified
/// tag abstraction (which doesn't expose CHAP frame timing data).
fn extract_id3_chapters(path: &Path) -> Option<Vec<RawChapter>> {
    let data = std::fs::read(path).ok()?;
    parse_id3v2_chap_frames(&data)
}

/// Scan raw file bytes for an ID3v2 tag and extract CHAP frames.
fn parse_id3v2_chap_frames(data: &[u8]) -> Option<Vec<RawChapter>> {
    // ID3v2 header: "ID3" + version(2) + flags(1) + size(4 syncsafe)
    if data.len() < 10 || &data[0..3] != b"ID3" {
        return None;
    }
    let tag_size = syncsafe_u32(&data[6..10]) as usize;
    let tag_end = (10 + tag_size).min(data.len());
    let mut pos = 10;

    let mut chapters = Vec::new();

    while pos + 10 <= tag_end {
        let frame_id = &data[pos..pos + 4];
        let frame_size = u32::from_be_bytes(data[pos + 4..pos + 8].try_into().ok()?) as usize;
        let content_start = pos + 10; // 4 id + 4 size + 2 flags
        let content_end = content_start + frame_size;

        if frame_size == 0 || content_end > tag_end {
            break;
        }

        if frame_id == b"CHAP" {
            if let Some(ch) = parse_chap_frame_body(&data[content_start..content_end]) {
                chapters.push(ch);
            }
        }

        pos = content_end;
    }

    if chapters.is_empty() {
        return None;
    }
    chapters.sort_by_key(|c| c.start_ms);
    Some(chapters)
}

fn syncsafe_u32(data: &[u8]) -> u32 {
    ((data[0] as u32) << 21) | ((data[1] as u32) << 14) | ((data[2] as u32) << 7) | (data[3] as u32)
}

/// Parse a CHAP frame body: element_id(null-terminated) + start(u32 ms) +
/// end(u32 ms) + start_offset(u32) + end_offset(u32) + sub-frames.
fn parse_chap_frame_body(data: &[u8]) -> Option<RawChapter> {
    let null_pos = data.iter().position(|&b| b == 0)?;
    if data.len() < null_pos + 1 + 16 {
        return None;
    }
    let t = null_pos + 1;
    let start_ms = u32::from_be_bytes(data[t..t + 4].try_into().ok()?) as u64;
    let end_ms = u32::from_be_bytes(data[t + 4..t + 8].try_into().ok()?) as u64;

    let sub_start = t + 16;
    let title = extract_tit2_from_subframes(&data[sub_start..]).unwrap_or_default();

    Some(RawChapter {
        title,
        start_ms,
        end_ms,
    })
}

/// Scan sub-frames for a TIT2 frame and decode its text.
fn extract_tit2_from_subframes(data: &[u8]) -> Option<String> {
    let mut pos = 0;
    while pos + 10 <= data.len() {
        let frame_id = &data[pos..pos + 4];
        let frame_size = u32::from_be_bytes(data[pos + 4..pos + 8].try_into().ok()?) as usize;
        let content_start = pos + 10;
        let content_end = content_start + frame_size;

        if frame_size == 0 || content_end > data.len() {
            break;
        }

        if frame_id == b"TIT2" && frame_size > 1 {
            let encoding = data[content_start];
            let text_bytes = &data[content_start + 1..content_end];
            return Some(decode_id3_text(encoding, text_bytes));
        }

        pos = content_end;
    }
    None
}

/// Decode ID3v2 text given the encoding byte.
fn decode_id3_text(encoding: u8, data: &[u8]) -> String {
    match encoding {
        0 => data
            .iter()
            .take_while(|&&b| b != 0)
            .map(|&b| b as char)
            .collect(),
        1 | 2 => {
            if data.len() < 2 {
                return String::new();
            }
            let (is_be, skip) = if data[0] == 0xFF && data[1] == 0xFE {
                (false, 2)
            } else if data[0] == 0xFE && data[1] == 0xFF {
                (true, 2)
            } else {
                (encoding == 2, 0)
            };
            let codepoints: Vec<u16> = data[skip..]
                .chunks_exact(2)
                .map(|p| {
                    if is_be {
                        u16::from_be_bytes([p[0], p[1]])
                    } else {
                        u16::from_le_bytes([p[0], p[1]])
                    }
                })
                .take_while(|&c| c != 0)
                .collect();
            String::from_utf16_lossy(&codepoints)
        }
        3 => {
            let end = data.iter().position(|&b| b == 0).unwrap_or(data.len());
            String::from_utf8_lossy(&data[..end]).to_string()
        }
        _ => String::from_utf8_lossy(data).to_string(),
    }
}

#[cfg(test)]
mod tests;
