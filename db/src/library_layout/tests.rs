//! Unit tests for the `library_layout` module.

use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

fn temp_dir(suffix: &str) -> PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let pid = std::process::id();
    let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("omnibus_layout_{suffix}_{pid}_{seq}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

#[test]
fn slugify_basic_ascii() {
    assert_eq!(slugify("Hello World"), "hello-world");
}

#[test]
fn slugify_strips_punctuation() {
    assert_eq!(slugify("What?! Really..."), "what-really");
}

#[test]
fn slugify_collapses_runs() {
    assert_eq!(slugify("a---b___c"), "a-b-c");
}

#[test]
fn slugify_trims_leading_and_trailing() {
    assert_eq!(slugify("---trim---"), "trim");
}

#[test]
fn slugify_folds_accents() {
    assert_eq!(slugify("Café au Lait"), "cafe-au-lait");
}

#[test]
fn slugify_transliterates_cjk() {
    // Locks in deunicode's transliteration. The exact letters matter less
    // than the fact that the result is non-empty ASCII.
    let out = slugify("東京物語");
    assert!(!out.is_empty(), "got empty slug for CJK input");
    assert!(
        out != FALLBACK_SLUG,
        "expected real transliteration, got fallback"
    );
    assert!(
        out.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "got non-ASCII in slug: {out:?}"
    );
}

#[test]
fn slugify_handles_cyrillic() {
    let out = slugify("Война и мир");
    assert!(!out.is_empty());
    assert!(
        out.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'),
        "got non-ASCII in slug: {out:?}"
    );
}

#[test]
fn slugify_empty_input_falls_back() {
    assert_eq!(slugify(""), "book");
}

#[test]
fn slugify_all_punctuation_falls_back() {
    assert_eq!(slugify("!!!???"), "book");
}

#[test]
fn slugify_caps_at_80_chars() {
    let long = "a".repeat(200);
    let out = slugify(&long);
    assert_eq!(out.len(), MAX_SLUG_LEN);
    assert!(out.is_char_boundary(out.len()));
}

#[test]
fn slugify_preserves_digits() {
    assert_eq!(slugify("Volume 2: The Sequel"), "volume-2-the-sequel");
}

#[test]
fn slugify_cap_does_not_leave_trailing_dash() {
    // 79 letters then a separator: cap at 80 must not stop right on the
    // dash and leave it dangling.
    let s = format!("{}-tail", "a".repeat(79));
    let out = slugify(&s);
    assert!(!out.ends_with('-'), "got trailing dash: {out:?}");
    assert!(out.len() <= MAX_SLUG_LEN);
}

#[test]
fn canonical_path_typical() {
    let p = canonical_path(
        Path::new("/lib"),
        "Brandon Sanderson",
        "The Way of Kings",
        "epub",
    );
    assert_eq!(
        p,
        PathBuf::from("/lib/brandon-sanderson/the-way-of-kings/the-way-of-kings.epub")
    );
}

#[test]
fn canonical_path_apostrophe() {
    let p = canonical_path(
        Path::new("/lib"),
        "Madeleine L'Engle",
        "A Wrinkle in Time",
        "epub",
    );
    assert_eq!(
        p,
        PathBuf::from("/lib/madeleine-l-engle/a-wrinkle-in-time/a-wrinkle-in-time.epub")
    );
}

#[test]
fn canonical_path_unicode_author() {
    let p = canonical_path(Path::new("/lib"), "村上春樹", "Norwegian Wood", "epub");
    // Author slug must be non-empty ASCII (deunicode-folded), and must
    // not be the fallback.
    let comps: Vec<_> = p
        .components()
        .map(|c| c.as_os_str().to_string_lossy().to_string())
        .collect();
    let author_seg = &comps[2];
    assert_ne!(author_seg, "book", "unicode author folded to fallback");
    assert!(author_seg
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-'));
}

#[test]
fn canonical_path_empty_author_falls_back() {
    let p = canonical_path(Path::new("/lib"), "", "Some Title", "epub");
    assert!(p.starts_with(Path::new("/lib/book/")));
}

#[test]
fn canonical_path_strips_leading_dot_in_ext() {
    let p = canonical_path(Path::new("/lib"), "A B", "T", ".EPUB");
    assert!(p.to_string_lossy().ends_with("/t.epub"));
}

#[test]
fn canonical_path_empty_ext_drops_trailing_dot() {
    // A trailing `.` is invalid on Windows and weird on POSIX. With an
    // empty `ext`, the filename is just the title slug.
    let p = canonical_path(Path::new("/lib"), "Author", "Title", "");
    assert!(
        p.to_string_lossy().ends_with("/title"),
        "got {}",
        p.display()
    );
    assert!(!p.to_string_lossy().ends_with('.'));
}

#[test]
fn canonical_path_lone_dot_ext_drops_trailing_dot() {
    let p = canonical_path(Path::new("/lib"), "Author", "Title", ".");
    assert!(p.to_string_lossy().ends_with("/title"));
    assert!(!p.to_string_lossy().ends_with('.'));
}

#[test]
fn sidecar_cover_for_per_stem_jpg() {
    let dir = temp_dir("per_stem_jpg");
    let epub = dir.join("book.epub");
    std::fs::write(&epub, b"").unwrap();
    let jpg = dir.join("book.jpg");
    std::fs::write(&jpg, b"x").unwrap();
    let got = sidecar_cover_for(&epub);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got, Some(jpg));
}

#[test]
fn sidecar_cover_for_per_stem_png() {
    let dir = temp_dir("per_stem_png");
    let epub = dir.join("book.epub");
    std::fs::write(&epub, b"").unwrap();
    let png = dir.join("book.png");
    std::fs::write(&png, b"x").unwrap();
    let got = sidecar_cover_for(&epub);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got, Some(png));
}

#[test]
fn sidecar_cover_for_per_stem_jpeg() {
    let dir = temp_dir("per_stem_jpeg");
    let epub = dir.join("book.epub");
    std::fs::write(&epub, b"").unwrap();
    let jpeg = dir.join("book.jpeg");
    std::fs::write(&jpeg, b"x").unwrap();
    let got = sidecar_cover_for(&epub);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got, Some(jpeg));
}

#[test]
fn sidecar_cover_for_falls_back_to_folder_cover() {
    let dir = temp_dir("folder_cover");
    let epub = dir.join("book.epub");
    std::fs::write(&epub, b"").unwrap();
    let cover = dir.join("cover.jpg");
    std::fs::write(&cover, b"x").unwrap();
    let got = sidecar_cover_for(&epub);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got, Some(cover));
}

#[test]
fn sidecar_cover_for_prefers_per_stem_over_folder() {
    let dir = temp_dir("per_stem_wins");
    let epub = dir.join("book.epub");
    std::fs::write(&epub, b"").unwrap();
    let stem = dir.join("book.jpg");
    std::fs::write(&stem, b"stem").unwrap();
    std::fs::write(dir.join("cover.jpg"), b"folder").unwrap();
    let got = sidecar_cover_for(&epub);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got, Some(stem));
}

#[test]
fn sidecar_cover_for_case_insensitive() {
    // The case-insensitive matching contract: a `Cover.JPG` on disk is
    // resolved even though the probed/canonical name is lowercase.
    // We compare via `canonicalize` rather than path strings because on
    // case-insensitive filesystems (APFS, NTFS) the direct-path fast path
    // returns the probed lowercase casing while on case-sensitive
    // filesystems (ext4) the `read_dir` fallback returns the original
    // `Cover.JPG` casing. Both spellings resolve to the same inode and
    // are functionally equivalent — see the `sidecar_cover_for` docstring.
    let dir = temp_dir("case");
    let epub = dir.join("book.epub");
    std::fs::write(&epub, b"").unwrap();
    let upper = dir.join("Cover.JPG");
    std::fs::write(&upper, b"x").unwrap();
    let got = sidecar_cover_for(&epub).expect("cover should be found");
    let got_canon = std::fs::canonicalize(&got).expect("canonicalize got");
    let upper_canon = std::fs::canonicalize(&upper).expect("canonicalize upper");
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got_canon, upper_canon);
}

#[test]
fn sidecar_cover_for_priority_jpg_over_png() {
    let dir = temp_dir("priority");
    let epub = dir.join("book.epub");
    std::fs::write(&epub, b"").unwrap();
    let jpg = dir.join("book.jpg");
    std::fs::write(&jpg, b"x").unwrap();
    std::fs::write(dir.join("book.png"), b"x").unwrap();
    let got = sidecar_cover_for(&epub);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got, Some(jpg));
}

#[test]
fn sidecar_cover_for_direct_path_fast_path() {
    // The common canonical-layout case: a lowercase per-stem `<stem>.jpg`
    // resolves via the direct-path `is_file()` fast path. We can't observe
    // the syscall count from here, but we can assert the contract still
    // returns the lowercase file.
    let dir = temp_dir("direct_path");
    let epub = dir.join("book.epub");
    std::fs::write(&epub, b"").unwrap();
    let jpg = dir.join("book.jpg");
    std::fs::write(&jpg, b"x").unwrap();
    let got = sidecar_cover_for(&epub);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got, Some(jpg));
}

#[test]
fn sidecar_cover_for_direct_path_per_stem_beats_ci_folder_cover() {
    // Ordering invariant: a direct-path per-stem sidecar must win over a
    // case-insensitive folder-level `Cover.JPG`. The direct-path fast path
    // is per-stem-first, so the folder `cover.*` (whether probed directly
    // or via the `read_dir` fallback) must never preempt a per-stem match.
    let dir = temp_dir("direct_stem_vs_ci_folder");
    let epub = dir.join("book.epub");
    std::fs::write(&epub, b"").unwrap();
    let stem = dir.join("book.jpg");
    std::fs::write(&stem, b"stem").unwrap();
    std::fs::write(dir.join("Cover.JPG"), b"folder").unwrap();
    let got = sidecar_cover_for(&epub);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got, Some(stem));
}

#[test]
fn sidecar_cover_for_ci_per_stem_beats_direct_folder_cover() {
    // Ordering invariant in the other direction: a per-stem sidecar
    // (uppercase `Book.JPG`) must still beat the folder-level `cover.jpg`.
    // The per-stem direct-path probe runs first against lowercase
    // `book.jpg` — on case-sensitive filesystems that probe misses and
    // the read_dir fallback finds `Book.JPG`; on case-insensitive ones
    // the probe succeeds and resolves to the same inode. In both cases
    // the returned path's contents must be the per-stem sidecar, not
    // the folder cover. We compare via file contents rather than path
    // strings to be filesystem-casing-agnostic.
    let dir = temp_dir("ci_stem_vs_direct_folder");
    let epub = dir.join("book.epub");
    std::fs::write(&epub, b"").unwrap();
    let stem = dir.join("Book.JPG");
    std::fs::write(&stem, b"stem").unwrap();
    std::fs::write(dir.join("cover.jpg"), b"folder").unwrap();
    let got = sidecar_cover_for(&epub).expect("cover should be found");
    let got_bytes = std::fs::read(&got).expect("read got");
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got_bytes, b"stem");
}

#[test]
fn sidecar_cover_for_no_match_returns_none() {
    let dir = temp_dir("no_match");
    let epub = dir.join("book.epub");
    std::fs::write(&epub, b"").unwrap();
    std::fs::write(dir.join("notes.txt"), b"x").unwrap();
    let got = sidecar_cover_for(&epub);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got, None);
}

#[cfg(unix)]
#[test]
fn sidecar_cover_for_non_utf8_stem_still_falls_back_to_cover() {
    // A path whose stem isn't valid UTF-8 should still pick up a
    // folder-level `cover.jpg` — the per-stem tier needs UTF-8 for the
    // case-insensitive compare, but the fallback doesn't.
    //
    // The epub path itself is constructed in-memory and doesn't need to
    // exist on disk (this also dodges macOS's APFS rejection of
    // non-UTF-8 filenames). Only the parent dir + cover.jpg need to
    // exist, since the function reads the parent.
    use std::ffi::OsStr;
    use std::os::unix::ffi::OsStrExt;

    let dir = temp_dir("non_utf8_stem");
    let cover = dir.join("cover.jpg");
    std::fs::write(&cover, b"x").unwrap();
    // 0xFF is invalid UTF-8 in any leading byte position. Build the
    // path by concatenating bytes onto the dir's OsStr.
    let mut bad_path_bytes = dir.as_os_str().as_bytes().to_vec();
    bad_path_bytes.extend_from_slice(b"/\xff\xff.epub");
    let epub = std::path::PathBuf::from(OsStr::from_bytes(&bad_path_bytes));

    let got = sidecar_cover_for(&epub);
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(got, Some(cover));
}

#[test]
fn allocate_no_collision_returns_canonical() {
    let dir = temp_dir("alloc_clean");
    let p = allocate_canonical_path(&dir, "Author A", "Title T", "epub").unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let s = p.to_string_lossy();
    assert!(s.ends_with("/author-a/title-t/title-t.epub"), "got: {s}");
}

#[test]
fn allocate_one_collision_appends_2() {
    let dir = temp_dir("alloc_one");
    std::fs::create_dir_all(dir.join("author-a").join("title-t")).unwrap();
    let p = allocate_canonical_path(&dir, "Author A", "Title T", "epub").unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let s = p.to_string_lossy();
    assert!(
        s.ends_with("/author-a/title-t (2)/title-t.epub"),
        "got: {s}"
    );
}

#[test]
fn allocate_empty_ext_is_invalid_input() {
    let dir = temp_dir("alloc_empty_ext");
    let result = allocate_canonical_path(&dir, "Author", "Title", "");
    std::fs::remove_dir_all(&dir).unwrap();
    let err = result.expect_err("empty ext must be rejected");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn allocate_lone_dot_ext_is_invalid_input() {
    let dir = temp_dir("alloc_lone_dot");
    let result = allocate_canonical_path(&dir, "Author", "Title", ".");
    std::fs::remove_dir_all(&dir).unwrap();
    let err = result.expect_err("lone dot must be rejected after stripping");
    assert_eq!(err.kind(), std::io::ErrorKind::InvalidInput);
}

#[test]
fn allocate_three_collisions_returns_4() {
    let dir = temp_dir("alloc_three");
    let author = dir.join("author-a");
    std::fs::create_dir_all(author.join("title-t")).unwrap();
    std::fs::create_dir_all(author.join("title-t (2)")).unwrap();
    std::fs::create_dir_all(author.join("title-t (3)")).unwrap();
    let p = allocate_canonical_path(&dir, "Author A", "Title T", "epub").unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    let s = p.to_string_lossy();
    assert!(
        s.ends_with("/author-a/title-t (4)/title-t.epub"),
        "got: {s}"
    );
}
