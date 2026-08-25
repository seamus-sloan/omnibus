//! Chapter extraction from audiobook containers: Nero `chpl` atoms and
//! QuickTime chapter tracks for M4B/M4A, ID3v2 CHAP frames for MP3. Returns
//! [`RawChapter`] entries the sync layer converts to absolute-timeline
//! `file_chapters` rows.

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

/// Walk the MP4 box tree for chapters, preferring the Nero `chpl` atom and
/// falling back to a QuickTime chapter track. Retail M4Bs — Audible/AAX
/// conversions, anything ffmpeg muxed — commonly carry only the latter.
fn extract_mp4_chapters(path: &Path) -> Option<Vec<RawChapter>> {
    let mut file = std::fs::File::open(path).ok()?;
    let file_len = file.metadata().ok()?.len();
    let moov = find_box(&mut file, file_len, b"moov")?;
    extract_nero_chapters(&mut file, &moov).or_else(|| extract_chapter_track(&mut file, &moov))
}

/// Nero chapters: a single `chpl` atom under `moov/udta`.
fn extract_nero_chapters(file: &mut std::fs::File, moov: &BoxInfo) -> Option<Vec<RawChapter>> {
    let udta = find_child_box(file, moov.data_offset, moov.data_size, b"udta")?;
    let chpl = find_child_box(file, udta.data_offset, udta.data_size, b"chpl")?;
    parse_chpl_payload(file, chpl.data_offset, chpl.data_size)
}

/// Upper bound on chapters returned from either MP4 path, matching the
/// `chpl` cap so a corrupt sample table can't be turned into an allocation.
const MAX_CHAPTERS: usize = 10_000;

/// Upper bound on entries in a sample table. Generous — a chapter track has
/// one sample per chapter — but keeps a bogus count from sizing a `Vec`.
const MAX_TABLE_ENTRIES: usize = 1_000_000;

/// Longest text sample we read; titles are short and the rest of a sample is
/// trailing atoms we ignore.
const MAX_SAMPLE_BYTES: u64 = 64 * 1024;

type BoxType = [u8; 4];

struct BoxInfo {
    data_offset: u64,
    data_size: u64,
}

/// Find a box at the current level starting from offset 0.
fn find_box(file: &mut std::fs::File, file_len: u64, target: &BoxType) -> Option<BoxInfo> {
    find_child_box(file, 0, file_len, target)
}

/// Find a child box within a parent box's data region.
fn find_child_box(
    file: &mut std::fs::File,
    parent_offset: u64,
    parent_size: u64,
    target: &BoxType,
) -> Option<BoxInfo> {
    list_child_boxes(file, parent_offset, parent_size)
        .into_iter()
        .find(|(box_type, _)| box_type == target)
        .map(|(_, info)| info)
}

/// List every child box within a parent box's data region, in file order.
/// Stops at the first structurally invalid box and returns what it gathered,
/// so a malformed trailing sibling can't hide the boxes ahead of it.
fn list_child_boxes(
    file: &mut std::fs::File,
    parent_offset: u64,
    parent_size: u64,
) -> Vec<(BoxType, BoxInfo)> {
    let mut boxes = Vec::new();
    let Some(end) = parent_offset.checked_add(parent_size) else {
        return boxes;
    };
    let mut pos = parent_offset;

    while let Some((box_type, info, box_size)) = read_box_header(file, pos, end) {
        boxes.push((box_type, info));
        // `box_size` is always at least the header length, so this advances;
        // the guard is belt-and-braces against an infinite walk.
        match pos.checked_add(box_size) {
            Some(next) if next > pos => pos = next,
            _ => break,
        }
    }
    boxes
}

/// Parse one box header, returning its type, payload location, and total
/// size. `None` when the header is truncated or its declared size overflows
/// or underruns.
fn read_box_header(
    file: &mut std::fs::File,
    pos: u64,
    end: u64,
) -> Option<(BoxType, BoxInfo, u64)> {
    // Adversarial size fields are attacker-controlled u64s, so every offset
    // arithmetic uses checked ops: a box claiming a length that overflows or
    // underruns its own header bails out rather than panicking mid-scan.
    if pos.checked_add(8)? > end {
        return None;
    }
    file.seek(SeekFrom::Start(pos)).ok()?;
    let mut header = [0u8; 8];
    file.read_exact(&mut header).ok()?;

    let size = u64::from(u32::from_be_bytes([
        header[0], header[1], header[2], header[3],
    ]));
    let mut box_type = [0u8; 4];
    box_type.copy_from_slice(&header[4..8]);

    let (box_size, data_offset) = if size == 1 {
        // 64-bit extended size
        let mut ext = [0u8; 8];
        file.read_exact(&mut ext).ok()?;
        (u64::from_be_bytes(ext), pos.checked_add(16)?)
    } else if size == 0 {
        // Box extends to end of file
        (end.checked_sub(pos)?, pos.checked_add(8)?)
    } else {
        (size, pos.checked_add(8)?)
    };

    // Reject a box whose declared size doesn't cover its own header (guards
    // the subtraction below) or that runs past the parent region.
    let header_len = data_offset.checked_sub(pos)?;
    if box_size < header_len || pos.checked_add(box_size)? > end {
        return None;
    }

    Some((
        box_type,
        BoxInfo {
            data_offset,
            data_size: box_size - header_len,
        },
        box_size,
    ))
}

/// Parse the Nero `chpl` atom payload into chapter entries.
///
/// Format: version (1 byte) + flags (3 bytes) + entry count (4 bytes for
/// v0, 1 byte for v1) + N entries of (start_100ns: u64, title_len: u8,
/// title: [u8; title_len]).
fn parse_chpl_payload(file: &mut std::fs::File, offset: u64, size: u64) -> Option<Vec<RawChapter>> {
    if size < 9 {
        return None;
    }
    file.seek(SeekFrom::Start(offset)).ok()?;
    let mut ver_flags = [0u8; 4];
    file.read_exact(&mut ver_flags).ok()?;
    let version = ver_flags[0];

    let count = if version >= 1 {
        let mut _reserved = [0u8; 4];
        file.read_exact(&mut _reserved).ok()?;
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

/// QuickTime chapters: the audio track's `tref`/`chap` names a sibling text
/// track whose samples carry the titles, timed by that track's own sample
/// tables. Resolving through `tref` rather than picking the first text track
/// keeps a subtitle track from being mistaken for chapters.
fn extract_chapter_track(file: &mut std::fs::File, moov: &BoxInfo) -> Option<Vec<RawChapter>> {
    let traks: Vec<BoxInfo> = list_child_boxes(file, moov.data_offset, moov.data_size)
        .into_iter()
        .filter(|(box_type, _)| box_type == b"trak")
        .map(|(_, info)| info)
        .collect();

    let referenced = chapter_track_ref(file, &traks)?;
    let mut chapter_trak = None;
    for trak in &traks {
        if track_id(file, trak) == Some(referenced) {
            chapter_trak = Some(trak);
            break;
        }
    }
    read_chapter_track(file, chapter_trak?)
}

/// The track id named by the first `tref`/`chap` reference found. A `chap`
/// box may list several ids; the first is the chapter track.
fn chapter_track_ref(file: &mut std::fs::File, traks: &[BoxInfo]) -> Option<u32> {
    for trak in traks {
        let Some(tref) = find_child_box(file, trak.data_offset, trak.data_size, b"tref") else {
            continue;
        };
        let Some(chap) = find_child_box(file, tref.data_offset, tref.data_size, b"chap") else {
            continue;
        };
        if let Some(id) = read_u32_in_box(file, &chap, 0) {
            return Some(id);
        }
    }
    None
}

/// A track's id from its `tkhd`. Version 1 carries 64-bit creation and
/// modification times ahead of the id, version 0 32-bit ones.
fn track_id(file: &mut std::fs::File, trak: &BoxInfo) -> Option<u32> {
    let tkhd = find_child_box(file, trak.data_offset, trak.data_size, b"tkhd")?;
    let offset = if box_version(file, &tkhd)? == 1 {
        20
    } else {
        12
    };
    read_u32_in_box(file, &tkhd, offset)
}

/// Read a chapter track's samples into chapters on the file-relative
/// timeline.
fn read_chapter_track(file: &mut std::fs::File, trak: &BoxInfo) -> Option<Vec<RawChapter>> {
    let mdia = find_child_box(file, trak.data_offset, trak.data_size, b"mdia")?;

    // A `tref` pointing at the wrong track would otherwise have us decoding
    // audio samples as text.
    let handler = media_handler(file, &mdia)?;
    if &handler != b"text" && &handler != b"sbtl" {
        return None;
    }

    let timescale = media_timescale(file, &mdia)?;
    let minf = find_child_box(file, mdia.data_offset, mdia.data_size, b"minf")?;
    let stbl = find_child_box(file, minf.data_offset, minf.data_size, b"stbl")?;

    let times = sample_times(file, &stbl, timescale)?;
    let locations = sample_locations(file, &stbl)?;

    // A track whose tables disagree on sample count is still usable for the
    // samples both describe.
    let count = times.len().min(locations.len());
    if count == 0 || count > MAX_CHAPTERS {
        return None;
    }

    let mut chapters = Vec::with_capacity(count);
    for (&(start_ms, end_ms), &(offset, size)) in times.iter().zip(locations.iter()).take(count) {
        chapters.push(RawChapter {
            title: read_text_sample(file, offset, size).unwrap_or_default(),
            start_ms,
            end_ms,
        });
    }
    Some(chapters)
}

/// The four-character handler type from `mdia/hdlr`.
fn media_handler(file: &mut std::fs::File, mdia: &BoxInfo) -> Option<BoxType> {
    let hdlr = find_child_box(file, mdia.data_offset, mdia.data_size, b"hdlr")?;
    // version+flags, then a pre-defined word, then the handler type.
    read_u32_in_box(file, &hdlr, 8).map(u32::to_be_bytes)
}

/// The track's own timescale from `mdia/mdhd`, in units per second.
fn media_timescale(file: &mut std::fs::File, mdia: &BoxInfo) -> Option<u32> {
    let mdhd = find_child_box(file, mdia.data_offset, mdia.data_size, b"mdhd")?;
    let offset = if box_version(file, &mdhd)? == 1 {
        20
    } else {
        12
    };
    // A zero timescale would divide by zero when converting to milliseconds.
    read_u32_in_box(file, &mdhd, offset).filter(|ts| *ts > 0)
}

/// Per-sample `(start_ms, end_ms)` from `stts`, whose entries are
/// run-length-encoded `(count, delta)` pairs in the track's timescale. Each
/// sample ends where the next begins, so chapters tile the timeline; the last
/// one ends at the track's total duration.
fn sample_times(
    file: &mut std::fs::File,
    stbl: &BoxInfo,
    timescale: u32,
) -> Option<Vec<(u64, u64)>> {
    let stts = find_child_box(file, stbl.data_offset, stbl.data_size, b"stts")?;
    let table = read_table(file, &stts, 8)?;

    let mut starts = Vec::new();
    let mut cursor = 0u64;
    for entry in table.chunks_exact(8) {
        let count = be_u32(&entry[0..4]);
        let delta = u64::from(be_u32(&entry[4..8]));
        for _ in 0..count {
            // Bounded here rather than by `count`, which a corrupt table can
            // set to u32::MAX.
            if starts.len() >= MAX_CHAPTERS {
                return None;
            }
            starts.push(cursor);
            cursor = cursor.checked_add(delta)?;
        }
    }

    let mut times = Vec::with_capacity(starts.len());
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(cursor);
        times.push((ticks_to_ms(start, timescale)?, ticks_to_ms(end, timescale)?));
    }
    Some(times)
}

/// Convert a time in track ticks to milliseconds. Widened to `u128` so a
/// large tick count times 1000 can't overflow before the divide.
fn ticks_to_ms(ticks: u64, timescale: u32) -> Option<u64> {
    u64::try_from(u128::from(ticks) * 1000 / u128::from(timescale)).ok()
}

/// Absolute `(offset, size)` for every sample, resolved from the chunk-offset,
/// sample-to-chunk, and sample-size tables.
fn sample_locations(file: &mut std::fs::File, stbl: &BoxInfo) -> Option<Vec<(u64, u64)>> {
    let sizes = sample_sizes(file, stbl)?;
    let chunks = chunk_offsets(file, stbl)?;
    let stsc = find_child_box(file, stbl.data_offset, stbl.data_size, b"stsc")?;
    let runs: Vec<(u32, u32)> = read_table(file, &stsc, 12)?
        .chunks_exact(12)
        .map(|entry| (be_u32(&entry[0..4]), be_u32(&entry[4..8])))
        .collect();

    let mut locations = Vec::new();
    let mut index = 0usize;
    for (i, &chunk_offset) in chunks.iter().enumerate() {
        // `stsc` runs are 1-based and sparse: an entry holds until the next
        // one's `first_chunk`, so this chunk's count is the last run starting
        // at or before it.
        let per_chunk = runs
            .iter()
            .take_while(|(first, _)| u64::from(*first) <= i as u64 + 1)
            .last()
            .map(|(_, n)| *n)?;

        let mut offset = chunk_offset;
        for _ in 0..per_chunk {
            // Exhausting `sizes` ends the walk, which also bounds this loop
            // against a `per_chunk` a corrupt table set to u32::MAX.
            let Some(&size) = sizes.get(index) else {
                return Some(locations);
            };
            locations.push((offset, size));
            offset = offset.checked_add(size)?;
            index += 1;
        }
        if index >= sizes.len() {
            break;
        }
    }
    Some(locations)
}

/// Per-sample byte sizes from `stsz`, which either states one uniform size
/// for every sample or lists them individually.
fn sample_sizes(file: &mut std::fs::File, stbl: &BoxInfo) -> Option<Vec<u64>> {
    let stsz = find_child_box(file, stbl.data_offset, stbl.data_size, b"stsz")?;
    let uniform = read_u32_in_box(file, &stsz, 4)?;
    let count = usize::try_from(read_u32_in_box(file, &stsz, 8)?).ok()?;
    if count > MAX_TABLE_ENTRIES {
        return None;
    }
    if uniform > 0 {
        return Some(vec![u64::from(uniform); count]);
    }

    let wanted = count.checked_mul(4)?;
    if u64::try_from(wanted).ok()? > stsz.data_size.checked_sub(12)? {
        return None;
    }
    let mut buf = vec![0u8; wanted];
    file.seek(SeekFrom::Start(stsz.data_offset.checked_add(12)?))
        .ok()?;
    file.read_exact(&mut buf).ok()?;
    Some(buf.chunks_exact(4).map(|e| u64::from(be_u32(e))).collect())
}

/// Chunk offsets from `stco` (32-bit) or `co64` (64-bit), whichever the track
/// carries.
fn chunk_offsets(file: &mut std::fs::File, stbl: &BoxInfo) -> Option<Vec<u64>> {
    if let Some(stco) = find_child_box(file, stbl.data_offset, stbl.data_size, b"stco") {
        return Some(
            read_table(file, &stco, 4)?
                .chunks_exact(4)
                .map(|e| u64::from(be_u32(e)))
                .collect(),
        );
    }
    let co64 = find_child_box(file, stbl.data_offset, stbl.data_size, b"co64")?;
    Some(
        read_table(file, &co64, 8)?
            .chunks_exact(8)
            .map(be_u64)
            .collect(),
    )
}

/// Read the entry table of a sample-table full box: version+flags, a u32
/// entry count, then `count` fixed-width records. Refuses a count the box is
/// too small to hold, so a corrupt value can't size an allocation.
fn read_table(file: &mut std::fs::File, info: &BoxInfo, entry_size: usize) -> Option<Vec<u8>> {
    let count = usize::try_from(read_u32_in_box(file, info, 4)?).ok()?;
    if count > MAX_TABLE_ENTRIES {
        return None;
    }
    let wanted = count.checked_mul(entry_size)?;
    if u64::try_from(wanted).ok()? > info.data_size.checked_sub(8)? {
        return None;
    }
    let mut buf = vec![0u8; wanted];
    file.seek(SeekFrom::Start(info.data_offset.checked_add(8)?))
        .ok()?;
    file.read_exact(&mut buf).ok()?;
    Some(buf)
}

/// Decode one QuickTime text sample: a big-endian u16 length, the title
/// bytes, then optional trailing atoms (`encd` and friends) we ignore.
fn read_text_sample(file: &mut std::fs::File, offset: u64, size: u64) -> Option<String> {
    if size < 2 {
        return None;
    }
    let wanted = usize::try_from(size.min(MAX_SAMPLE_BYTES)).ok()?;
    let mut buf = vec![0u8; wanted];
    file.seek(SeekFrom::Start(offset)).ok()?;
    file.read_exact(&mut buf).ok()?;

    // Trust the buffer over the declared length: a sample truncated by the
    // cap or by a short read yields the title we actually have.
    let declared = usize::from(u16::from_be_bytes([buf[0], buf[1]]));
    let end = declared.min(buf.len() - 2) + 2;
    Some(decode_text_sample(&buf[2..end]))
}

/// UTF-16 when the sample carries a byte-order mark, UTF-8 otherwise — the
/// two encodings QuickTime text samples use in practice.
fn decode_text_sample(bytes: &[u8]) -> String {
    match bytes {
        [0xFE, 0xFF, rest @ ..] => decode_utf16(rest, true),
        [0xFF, 0xFE, rest @ ..] => decode_utf16(rest, false),
        _ => String::from_utf8_lossy(bytes).to_string(),
    }
}

fn decode_utf16(bytes: &[u8], big_endian: bool) -> String {
    let units: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|p| {
            if big_endian {
                u16::from_be_bytes([p[0], p[1]])
            } else {
                u16::from_le_bytes([p[0], p[1]])
            }
        })
        .collect();
    String::from_utf16_lossy(&units)
}

/// The version byte of a full box — the first byte of its version+flags word.
fn box_version(file: &mut std::fs::File, info: &BoxInfo) -> Option<u8> {
    if info.data_size < 4 {
        return None;
    }
    file.seek(SeekFrom::Start(info.data_offset)).ok()?;
    let mut buf = [0u8; 1];
    file.read_exact(&mut buf).ok()?;
    Some(buf[0])
}

/// Read a big-endian u32 at `offset` bytes into a box's payload,
/// bounds-checked against the box's declared size.
fn read_u32_in_box(file: &mut std::fs::File, info: &BoxInfo, offset: u64) -> Option<u32> {
    if offset.checked_add(4)? > info.data_size {
        return None;
    }
    file.seek(SeekFrom::Start(info.data_offset.checked_add(offset)?))
        .ok()?;
    let mut buf = [0u8; 4];
    file.read_exact(&mut buf).ok()?;
    Some(u32::from_be_bytes(buf))
}

/// Big-endian u32 from the first four bytes, zero-padded when short.
fn be_u32(bytes: &[u8]) -> u32 {
    let mut buf = [0u8; 4];
    let n = bytes.len().min(4);
    buf[..n].copy_from_slice(&bytes[..n]);
    u32::from_be_bytes(buf)
}

/// Big-endian u64 from the first eight bytes, zero-padded when short.
fn be_u64(bytes: &[u8]) -> u64 {
    let mut buf = [0u8; 8];
    let n = bytes.len().min(8);
    buf[..n].copy_from_slice(&bytes[..n]);
    u64::from_be_bytes(buf)
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
    // ID3v2 header: "ID3" + major_version(1) + revision(1) + flags(1) + size(4 syncsafe)
    if data.len() < 10 || &data[0..3] != b"ID3" {
        return None;
    }
    let major_version = data[3];
    let tag_size = syncsafe_u32(&data[6..10]) as usize;
    let tag_end = (10 + tag_size).min(data.len());
    let mut pos = 10;

    let mut chapters = Vec::new();

    while pos + 10 <= tag_end {
        let frame_id = &data[pos..pos + 4];
        // v2.4 uses syncsafe integers for frame sizes; v2.3 uses plain BE u32
        let frame_size = if major_version >= 4 {
            syncsafe_u32(&data[pos + 4..pos + 8]) as usize
        } else {
            u32::from_be_bytes(data[pos + 4..pos + 8].try_into().ok()?) as usize
        };
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
pub(super) mod tests;
