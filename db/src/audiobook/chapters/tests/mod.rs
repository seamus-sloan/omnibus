//! Unit tests for chapter extraction, split by source into the sibling
//! modules below; the MP4 box builders and the chapter-track fixture (also
//! used by `audiobook::parse`'s tests) live here.

mod chapter_track;
mod hostile_input;
mod nero_id3;

use std::io::Write;

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

/// Wrap a body in an MP4 box header (`size(4 BE) + type(4) + body`).
fn box_with(box_type: &[u8; 4], body: &[u8]) -> Vec<u8> {
    let size = (8 + body.len()) as u32;
    let mut out = Vec::with_capacity(size as usize);
    out.extend_from_slice(&size.to_be_bytes());
    out.extend_from_slice(box_type);
    out.extend_from_slice(body);
    out
}

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
