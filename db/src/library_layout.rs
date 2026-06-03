//! Canonical Omnibus library layout helpers. Writes the tree as
//! `<library_root>/<author-slug>/<title-slug>/<title-slug>.<ext>` for the
//! upload path; the read path uses the tolerant scanner in
//! [`crate::scanner`] / [`crate::ebook`] plus the sidecar cover lookup
//! ([`sidecar_cover_for`]). Per-helper docs spell out slug + sidecar rules.

use std::path::{Path, PathBuf};

const MAX_SLUG_LEN: usize = 80;
const FALLBACK_SLUG: &str = "book";

/// ASCII-fold + lowercase + collapse non-alphanumerics into a single `-`.
/// Caps at 80 chars on a codepoint boundary. Empty result falls back to
/// `"book"`.
pub fn slugify(s: &str) -> String {
    let folded = deunicode::deunicode(s).to_ascii_lowercase();
    let mut out = String::with_capacity(folded.len().min(MAX_SLUG_LEN));
    let mut last_was_dash = true; // suppress leading dashes
    for ch in folded.chars() {
        if ch.is_ascii_alphanumeric() {
            // Codepoint-boundary cap. Since the post-deunicode string is ASCII,
            // every char is one byte, but checking len() keeps us correct if
            // deunicode ever returns a non-ASCII char.
            if out.len() + ch.len_utf8() > MAX_SLUG_LEN {
                break;
            }
            out.push(ch);
            last_was_dash = false;
        } else if !last_was_dash && out.len() < MAX_SLUG_LEN {
            out.push('-');
            last_was_dash = true;
        }
    }
    while out.ends_with('-') {
        out.pop();
    }
    if out.is_empty() {
        FALLBACK_SLUG.to_string()
    } else {
        out
    }
}

/// Compute the canonical on-disk path for a book without touching the
/// filesystem: `<root>/<author-slug>/<title-slug>/<title-slug>.<ext>`.
///
/// An empty `ext` produces a filename without an extension
/// (`<title-slug>`), not a trailing dot — paths ending in `.` are awkward on
/// POSIX and outright rejected by Windows.
pub fn canonical_path(library_root: &Path, author: &str, title: &str, ext: &str) -> PathBuf {
    let author_slug = slugify(author);
    let title_slug = slugify(title);
    let ext_clean = ext.trim_start_matches('.').to_ascii_lowercase();
    let filename = if ext_clean.is_empty() {
        title_slug.clone()
    } else {
        format!("{title_slug}.{ext_clean}")
    };
    library_root
        .join(&author_slug)
        .join(&title_slug)
        .join(filename)
}

/// Return the cover sidecar file path for `ebook_path`, if any. Looks first
/// for a per-stem sidecar (`<basename>.{jpg,jpeg,png}` next to the epub) and
/// falls back to a folder-level `cover.{jpg,jpeg,png}`. All filename matches
/// are case-insensitive. Within each tier, priority order is `.jpg` > `.jpeg`
/// > `.png`.
///
/// Lookup order, highest priority first:
/// 1. direct-path probe, per-stem (`<stem>.{jpg,jpeg,png}` via `is_file()`)
/// 2. case-insensitive per-stem (`read_dir` scan)
/// 3. direct-path probe, folder-level (`cover.{jpg,jpeg,png}` via `is_file()`)
/// 4. case-insensitive folder-level (`read_dir` scan)
///
/// The direct-path probes are a cheap fast path: on the canonical Omnibus
/// layout (and any library whose sidecars use lowercase names) they resolve
/// the cover with a few `stat` syscalls and skip `read_dir` entirely. This
/// matters on flat-dump libraries with thousands of files in one folder,
/// where each `read_dir` is O(files-in-folder). The case-insensitive
/// `read_dir` scans remain as fallbacks so the matching contract is unchanged
/// — the only observable difference is fewer syscalls on the common case.
///
/// Note: "direct-path probe" rather than "exact-case lookup" because
/// `is_file()` on a case-insensitive filesystem (APFS, NTFS) will match a
/// differently-cased file too. That's fine here — any match still satisfies
/// the documented case-insensitive contract — but the returned `PathBuf`
/// preserves the *probed* casing, not the on-disk entry casing. Callers
/// that need the on-disk casing should fall through to `find_with_extensions`,
/// which carries the entry casing back from `read_dir`.
pub fn sidecar_cover_for(ebook_path: &Path) -> Option<PathBuf> {
    let parent = ebook_path.parent()?;

    // Per-stem first (handles flat-dump layouts where one folder contains
    // many books and `cover.jpg` would be ambiguous), then folder-level.
    // Per-stem matching needs UTF-8 for the case-insensitive compare; if the
    // filename isn't UTF-8 we skip the per-stem tier but still fall back to
    // `cover.*` since that lookup doesn't depend on the ebook's name.
    //
    // Within the per-stem tier the direct-path `is_file()` probe runs before
    // the case-insensitive `read_dir` scan, so a lowercase `<stem>.jpg`
    // resolves without ever listing the folder. The folder-level `cover.*`
    // tier only runs after the per-stem tier has been exhausted (both its
    // direct-path and case-insensitive halves), preserving the
    // per-stem-over-folder precedence.
    if let Some(stem) = ebook_path.file_stem().and_then(|s| s.to_str()) {
        // Fast path: probe the direct paths before scanning the folder.
        if let Some(found) = probe_direct_path(parent, stem) {
            return Some(found);
        }
        if let Some(found) = find_with_extensions(parent, stem) {
            return Some(found);
        }
    }
    if let Some(found) = probe_direct_path(parent, "cover") {
        return Some(found);
    }
    find_with_extensions(parent, "cover")
}

const COVER_EXTS: &[&str] = &["jpg", "jpeg", "png"];

/// Direct-path probe: stat `<base>.{jpg,jpeg,png}` directly with `is_file()`
/// in `COVER_EXTS` priority order, avoiding a `read_dir` of the whole folder.
/// Returns the first match, or `None` if no probed path is a regular file
/// (caller then falls back to the case-insensitive `read_dir` scan).
///
/// "Direct-path" rather than "exact-case" because `is_file()` follows the
/// host filesystem's casing rules — on case-insensitive filesystems (APFS,
/// NTFS) the probe will succeed against a differently-cased on-disk entry.
/// That's fine: the caller's contract is already case-insensitive, and any
/// match returned here satisfies it.
fn probe_direct_path(dir: &Path, base: &str) -> Option<PathBuf> {
    for ext in COVER_EXTS {
        let candidate = dir.join(format!("{base}.{ext}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn find_with_extensions(dir: &Path, base: &str) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut best: Option<(usize, PathBuf)> = None;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(name) = path.file_stem().and_then(|s| s.to_str()) else {
            continue;
        };
        if !name.eq_ignore_ascii_case(base) {
            continue;
        }
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        if let Some(rank) = COVER_EXTS.iter().position(|e| e.eq_ignore_ascii_case(ext)) {
            if best.as_ref().is_none_or(|(r, _)| rank < *r) {
                best = Some((rank, path));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Compute a canonical path that doesn't already exist on disk. If the
/// canonical title-slug folder already exists, append ` (2)`, ` (3)`, … to
/// the title-slug component until an unused folder is found, and place the
/// file inside that suffixed folder.
///
/// Upload-time helper, kept covered by tests even though no caller wires
/// it in yet. An empty `ext` is rejected with `InvalidInput` — uploads
/// must know the format they're storing.
pub fn allocate_canonical_path(
    library_root: &Path,
    author: &str,
    title: &str,
    ext: &str,
) -> std::io::Result<PathBuf> {
    let author_slug = slugify(author);
    let title_slug = slugify(title);
    let ext_clean = ext.trim_start_matches('.').to_ascii_lowercase();
    if ext_clean.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "file extension must not be empty",
        ));
    }
    let author_dir = library_root.join(&author_slug);

    let mut suffix: u32 = 1;
    loop {
        let folder_name = if suffix == 1 {
            title_slug.clone()
        } else {
            format!("{title_slug} ({suffix})")
        };
        let candidate = author_dir.join(&folder_name);
        if !candidate.exists() {
            return Ok(candidate.join(format!("{title_slug}.{ext_clean}")));
        }
        suffix += 1;
        if suffix > 9999 {
            // Defensive: a real library will never see 10k collisions on one
            // title slug. Bail loudly rather than spin.
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("too many collisions for title slug {title_slug:?}"),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
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

    // ---------- slugify ----------

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

    // ---------- canonical_path ----------

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

    // ---------- sidecar_cover_for ----------

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

    // ---------- allocate_canonical_path ----------

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
}
