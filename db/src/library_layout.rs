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

/// Allocate a non-colliding canonical *folder* for a multi-file book (e.g. a
/// per-chapter `.mp3` audiobook): `<root>/<author-slug>/<title-slug>/`. Like
/// [`allocate_canonical_path`] the title-slug component gains a ` (2)`, ` (3)`,
/// … suffix until an unused folder is found, but the returned path is the
/// directory itself — the caller places each part inside it. Unlike the
/// single-file allocator this takes no extension: the parts keep their own
/// filenames within the folder.
pub fn allocate_canonical_dir(
    library_root: &Path,
    author: &str,
    title: &str,
) -> std::io::Result<PathBuf> {
    let author_slug = slugify(author);
    let title_slug = slugify(title);
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
            return Ok(candidate);
        }
        suffix += 1;
        if suffix > 9999 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::AlreadyExists,
                format!("too many collisions for title slug {title_slug:?}"),
            ));
        }
    }
}

const FALLBACK_SLUG_PART: &str = "part";

/// Sanitize a client-supplied filename into a safe `<stem-slug>.<ext>` basename
/// for placing inside a canonical folder. Strips any directory components
/// (defeating `../` path traversal), [`slugify`]s the stem, and lowercases the
/// extension so the audiobook scanner's extension filter still recognizes the
/// part. A missing/empty stem falls back to `"part"`; a missing extension
/// yields the bare stem slug. The `(track, filename)` playlist sort survives
/// because a leading-number stem (`01-intro`) still sorts first.
pub fn sanitize_part_filename(filename: &str) -> String {
    let base = Path::new(filename)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(filename);
    let path = Path::new(base);
    let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .map(str::to_ascii_lowercase);
    let stem_slug = if stem.is_empty() {
        FALLBACK_SLUG_PART.to_string()
    } else {
        slugify(stem)
    };
    match ext {
        Some(ext) if !ext.is_empty() => format!("{stem_slug}.{ext}"),
        _ => stem_slug,
    }
}

#[cfg(test)]
mod tests;
