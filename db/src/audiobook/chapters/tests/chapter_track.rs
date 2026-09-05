//! The QuickTime chapter track: timescale scaling, `co64` offsets, many
//! samples per chunk, UTF-16 and NUL-terminated titles, `chpl` preferred
//! over it, the dangling / non-text / missing-`tref` empties, version-1
//! header offsets, the sample cap, and graceful handling of truncation,
//! oversized tables, a zero timescale and offsets past EOF.

use super::super::*;
use super::{make_chapter_track_fixture, sample_chapters, temp_with_bytes, TrackLayout};

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
