use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};

use super::parse::AudiobookError;
use super::*;

fn make_test_dir(suffix: &str) -> std::path::PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("omnibus_audiobook_{suffix}_{pid}_{seq}"));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).expect("should create test dir");
    dir
}

#[test]
fn stat_with_no_path_returns_empty() {
    let out = stat_audiobook_library(None, "");
    assert!(out.entries.is_empty());
    assert!(out.path.is_none());
    assert!(out.error.is_none());
}

#[test]
fn stat_with_missing_path_reports_error() {
    let out = stat_audiobook_library(
        Some("/definitely/does/not/exist/omnibus_audiobook_test"),
        "/definitely/does/not/exist/omnibus_audiobook_test",
    );
    assert!(out.error.is_some());
    assert!(out.entries.is_empty());
}

#[test]
fn stat_picks_up_each_accepted_extension_and_skips_others() {
    let dir = make_test_dir("exts");
    fs::write(dir.join("a.m4b"), b"x").unwrap();
    fs::write(dir.join("b.m4a"), b"x").unwrap();
    fs::write(dir.join("c.mp3"), b"x").unwrap();
    fs::write(dir.join("d.txt"), b"x").unwrap();
    fs::write(dir.join("e.epub"), b"x").unwrap();
    let out = stat_audiobook_library(Some(dir.to_str().unwrap()), dir.to_str().unwrap());
    fs::remove_dir_all(&dir).unwrap();
    let names: Vec<_> = out.entries.iter().map(|e| e.filename.as_str()).collect();
    assert!(out.error.is_none());
    assert_eq!(names, vec!["a.m4b", "b.m4a", "c.mp3"]);
    // Same scan_key scheme as the ebook path — non-empty per real file.
    assert!(out.entries.iter().all(|e| !e.scan_key.is_empty()));
}

#[test]
fn stat_recurses_into_subdirectories() {
    let dir = make_test_dir("nested");
    let nested = dir.join("author").join("title");
    fs::create_dir_all(&nested).unwrap();
    fs::write(nested.join("book.m4b"), b"x").unwrap();
    let out = stat_audiobook_library(Some(dir.to_str().unwrap()), dir.to_str().unwrap());
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(out.entries.len(), 1);
    assert!(out.entries[0].filename.ends_with("book.m4b"));
    assert!(out.entries[0].filename.contains("author"));
}

#[test]
fn parse_unreadable_file_surfaces_as_error_metadata_row() {
    // Phase B's per-file failure shape: an audiobook that fails to parse
    // becomes an IndexedBook with `metadata.error = Some(...)`, never a
    // panic — same contract the EPUB path uses so one bad file doesn't
    // suppress the rest of the library.
    let dir = make_test_dir("badfile");
    let f = dir.join("broken.m4b");
    fs::write(&f, b"not actually an m4b").unwrap();
    let out = parse_audiobook_targets(vec![AudiobookParseTarget {
        filename: "broken.m4b".into(),
        absolute: f.clone(),
        mtime_epoch: 42,
        size_bytes: 19,
    }]);
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(out.len(), 1);
    let row = &out[0];
    assert!(row.metadata.error.is_some());
    assert_eq!(row.metadata.filename, "broken.m4b");
    assert_eq!(row.mtime_epoch, 42);
    assert_eq!(row.size_bytes, 19);
}

#[test]
fn parse_groups_emits_one_book_per_m4b_even_when_stems_share_a_base() {
    // Wind-and-Truth repro: distinct m4b files for the same book that
    // would have collided under the old filename-stripping heuristic
    // must now stay separate so part 5 isn't hidden inside part 3's row.
    // Title falls back to the per-file stem (tagless junk → no album).
    let dir = make_test_dir("m4b_parts");
    for name in ["Dracula Pt10.m4b", "Dracula Pt2.m4b", "Dracula Pt1.m4b"] {
        fs::write(dir.join(name), b"junk").unwrap();
    }
    let stat = stat_audiobook_library(dir.to_str(), dir.to_str().unwrap());
    let groups = group_into_books(stat.entries, dir.to_str().unwrap());
    assert_eq!(groups.len(), 3);
    let books = parse_groups(groups, &dir);
    fs::remove_dir_all(&dir).unwrap();
    assert_eq!(books.len(), 3);
    let titles: Vec<&str> = books.iter().map(|b| b.title.as_str()).collect();
    assert!(titles.contains(&"Dracula Pt1"));
    assert!(titles.contains(&"Dracula Pt2"));
    assert!(titles.contains(&"Dracula Pt10"));
    for b in &books {
        assert_eq!(b.parts.len(), 1);
        assert_eq!(b.parts[0].ordinal, 0);
    }
}

#[test]
fn audiobook_error_io_variant_renders_useful_message() {
    let io = std::io::Error::new(std::io::ErrorKind::NotFound, "missing.m4b");
    let err: AudiobookError = io.into();
    let s = err.to_string();
    assert!(s.starts_with("io error"), "got {s:?}");
}

#[test]
fn audiobook_error_unsupported_variant_renders_format_token() {
    let err = AudiobookError::Unsupported("xyz".into());
    let s = err.to_string();
    assert!(s.contains("xyz"), "got {s:?}");
}

#[test]
fn build_indexed_book_surfaces_tag_error_for_undecodable_audio() {
    // `build_indexed_book` propagates the lofty decode/container failure via
    // `?` as `AudiobookError::Tag` — the direct-propagation counterpart to
    // `parse_audiobook_targets`, which instead folds the same failure into an
    // `error` metadata row. Feeding a file with an audio extension but junk
    // bytes drives `lofty::read_from_path` to error, which `#[from]` maps to
    // `Tag`.
    let dir = make_test_dir("tag_err");
    let f = dir.join("garbage.m4b");
    fs::write(&f, b"this is not a valid m4b container").unwrap();
    let err =
        build_indexed_book(&f, "garbage.m4b".into()).expect_err("undecodable audio must not parse");
    fs::remove_dir_all(&dir).unwrap();
    assert!(matches!(err, AudiobookError::Tag(_)), "got {err:?}");
    assert!(
        err.to_string().starts_with("tag decode failed"),
        "got {err}"
    );
}

/// Absolute path to a committed (non-download-gated) generated audiobook
/// fixture under `test_data/audiobooks/generated/`.
fn generated_fixture(rel: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../test_data/audiobooks/generated")
        .join(rel)
}

#[test]
fn inspect_audiobook_files_single_mp3_reads_book_metadata() {
    let path = generated_fixture("ada_lovelace_solo/the_analytical_audiobook.mp3");
    let out = inspect_audiobook_files(&[path]).expect("fixture should parse");
    assert!(
        out.title.as_deref().is_some_and(|t| !t.is_empty()),
        "expected a title tag, got {:?}",
        out.title
    );
    assert!(
        out.duration_seconds.is_some_and(|d| d > 0.0),
        "expected a positive duration"
    );
}

#[test]
fn inspect_audiobook_files_multi_mp3_derives_title_from_album() {
    let parts = [
        generated_fixture("grace_hopper_series/the_compiled_tales/chapter01.mp3"),
        generated_fixture("grace_hopper_series/the_compiled_tales/chapter02.mp3"),
    ];
    let out = inspect_audiobook_files(&parts).expect("fixtures should parse");
    // Multi-file: the book title is the shared album tag, not either
    // chapter's per-file title tag.
    assert!(
        out.title.as_deref().is_some_and(|t| !t.is_empty()),
        "expected an album-derived title, got {:?}",
        out.title
    );
    // Duration is summed across both parts.
    assert!(out.duration_seconds.is_some_and(|d| d > 0.0));
}

#[test]
fn inspect_audiobook_files_errors_when_no_readable_tags() {
    let dir = make_test_dir("inspect_junk");
    let f = dir.join("junk.mp3");
    fs::write(&f, b"not an mp3 at all").unwrap();
    let err = inspect_audiobook_files(&[f]).expect_err("junk must not parse");
    fs::remove_dir_all(&dir).unwrap();
    assert!(matches!(err, AudiobookError::Unsupported(_)), "got {err:?}");
}

#[test]
fn duration_to_hm_splits_finite_seconds_into_hours_and_minutes() {
    assert_eq!(duration_to_hm(3661.0), (1, 1));
    assert_eq!(duration_to_hm(0.0), (0, 0));
    assert_eq!(duration_to_hm(59.0), (0, 0));
    assert_eq!(duration_to_hm(7200.0), (2, 0));
}

#[test]
fn duration_to_hm_returns_zero_for_non_finite_or_negative_input() {
    // The bug this guards against: an `f64 as i64` cast of a NaN/inf
    // duration produced `-9223372036854775808` in the description,
    // surfacing as a garbled `-9223372036854775808h …m` in the UI.
    assert_eq!(duration_to_hm(f64::NAN), (0, 0));
    assert_eq!(duration_to_hm(f64::INFINITY), (0, 0));
    assert_eq!(duration_to_hm(f64::NEG_INFINITY), (0, 0));
    assert_eq!(duration_to_hm(-1.0), (0, 0));
}

#[test]
fn duration_to_hm_clamps_implausibly_large_values_to_in_range_cast() {
    // f64 values larger than i32::MAX are clamped so the final `as i64`
    // cast cannot wrap or saturate-to-MIN.
    let (h, m) = duration_to_hm(f64::MAX);
    assert!(h >= 0 && m >= 0);
    assert!(h <= i64::from(i32::MAX) / 3600 + 1);
}
