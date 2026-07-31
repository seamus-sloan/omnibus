//! Ebook metadata extraction (server-only). Walks the configured library
//! directory, parses the OPF for each `.epub` (CBZ archives dispatch to
//! [`crate::comic`]), and produces an [`IndexedBook`] per file — metadata
//! plus the raw cover bytes. Parse failures surface as a per-book error
//! rather than hiding the rest of the library. Consumed by
//! [`crate::indexer`], which writes the output to the DB.

use std::path::Path;

use omnibus_shared::EbookMetadata;

mod accent;
mod cover;
mod parse;
mod stat;
mod wordcount;

pub use accent::extract_accent;
pub(crate) use cover::resolve_cover_with;
pub use parse::{parse_ebook_targets, ParseTarget};
pub use stat::{stat_ebook_library, StatEntry, StatScanResult};
pub use wordcount::estimate_word_count;

/// `book_files.format` values produced by the ebook indexer. Used by
/// [`crate::indexer::reindex`] to scope the diff's "currently indexed"
/// view to ebook rows — prevents `sync_books` from classifying foreign
/// (audiobook) rows under a shared `library_path` as Removed and
/// deleting them. Also doubles as the stat walk's extension allowlist
/// (compared case-insensitively), so the diff's view and the walk can
/// never disagree. Uppercased to match
/// [`crate::helpers::split_filename`]'s output.
pub const EBOOK_FORMATS: &[&str] = &["CBZ", "EPUB"];

/// A single scanner output row — metadata plus the raw cover image bytes
/// (and mime), if the epub included one. Consumed by [`crate::sync::sync_books`].
///
/// `mtime_epoch` and `size_bytes` are the filesystem stat values captured
/// during the walk, used by the incremental reindex diff to detect changes
/// and by the writer to persist on `book_files`.
///
/// `word_count` is the spine-text estimate ([`estimate_word_count`]), computed
/// once here while the EPUB is already open so `books.word_count` can feed the
/// stats Pages tile without re-parsing. `None` when the estimate could not be
/// produced (empty/unreadable spine).
#[derive(Debug, Default)]
pub struct IndexedBook {
    pub metadata: EbookMetadata,
    pub cover: Option<(String, Vec<u8>)>,
    pub mtime_epoch: i64,
    pub size_bytes: i64,
    pub word_count: Option<i64>,
}

/// Result of scanning an ebook library directory. Separate from
/// `EbookLibrary` (the API shape) because the scanner carries raw cover
/// bytes, not the `/api/covers/:id` URLs the API returns.
pub struct ScanResult {
    pub path: Option<String>,
    pub books: Vec<IndexedBook>,
    pub error: Option<String>,
}

/// Knobs that change how a scan touches the filesystem. Default keeps the
/// scan read-only; the indexer (production path) opts into
/// `materialize_sidecars` so subsequent scans hit `<basename>.jpg` directly
/// instead of re-opening the zip.
#[derive(Debug, Clone, Default)]
pub struct ScanOptions {
    /// A `cover.{jpg,jpeg,png}` (or per-stem `<basename>.{jpg,jpeg,png}`)
    /// sidecar next to the epub is always preferred over the embedded cover.
    /// With this set, on a successful cover extraction with no existing
    /// sidecar, write the embedded bytes to `<basename>.{jpg|png}` next to
    /// the epub so subsequent scans skip the zip entirely. Best-effort:
    /// errors are swallowed (logged via `tracing::warn!`) so a read-only
    /// filesystem doesn't kill the scan.
    pub materialize_sidecars: bool,
}

/// Scan an ebook library directory and extract EPUB metadata, using default `ScanOptions`.
pub fn scan_ebook_library(path: Option<&str>) -> ScanResult {
    scan_ebook_library_with(path, ScanOptions::default())
}

/// Full-walk-and-parse scan, kept for non-indexer callers (server bootstrap
/// probes, existing tests). Now a thin wrapper over the two phases —
/// `stat_ebook_library` → build all-entries target list → `parse_ebook_targets`.
///
/// The incremental reindex path in [`crate::indexer::reindex`] uses the two
/// phases directly so it can skip Phase B for unchanged files.
pub fn scan_ebook_library_with(path: Option<&str>, opts: ScanOptions) -> ScanResult {
    // For this legacy callpath, the library_path string is the input path
    // verbatim — same key the indexer uses, so stable_uuid stays consistent
    // whether you arrive via `scan_ebook_library_with` or the two-phase API.
    let library_path_key = path.unwrap_or_default();
    let stat = stat_ebook_library(path, library_path_key);
    let Some(root) = path else {
        return ScanResult {
            path: stat.path,
            books: vec![],
            error: stat.error,
        };
    };
    let dir = Path::new(root);
    // Skip the synthetic placeholders the stat walk emits for unreadable
    // subdirectories — Phase B should only see real ebook files.
    // Without this filter, parse_ebook_targets calls extract_metadata on
    // the directory path (wasted work + bogus parse-error row), and the
    // explicit error-row append below produces a duplicate.
    let targets: Vec<ParseTarget> = stat
        .entries
        .iter()
        .filter(|e| !e.scan_key.is_empty())
        .map(|e| ParseTarget {
            filename: e.filename.clone(),
            absolute: dir.join(&e.filename),
            mtime_epoch: e.mtime_epoch,
            size_bytes: e.size_bytes,
        })
        .collect();
    let mut books = parse_ebook_targets(targets, opts);
    // Surface unreadable subdirectory placeholders captured during the
    // stat walk so the legacy contract (one error row per unreadable
    // subdir) is preserved. The placeholder carries the original
    // io::Error string so callers can distinguish "permission denied"
    // from "no such file" — same diagnostic detail the pre-split
    // implementation surfaced.
    for placeholder in stat.entries.into_iter().filter(|e| e.scan_key.is_empty()) {
        let msg = placeholder
            .error
            .as_deref()
            .map(|e| format!("could not read directory: {e}"))
            .unwrap_or_else(|| "could not read directory".to_string());
        books.push(IndexedBook {
            metadata: EbookMetadata {
                filename: placeholder.filename.clone(),
                error: Some(msg),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
            word_count: None,
        });
    }
    books.sort_by(|a, b| a.metadata.filename.cmp(&b.metadata.filename));
    ScanResult {
        path: stat.path,
        books,
        error: stat.error,
    }
}

#[cfg(test)]
pub(crate) mod test_support {
    use std::path::Path;

    // `make_test_dir` lives in the crate-level `test_support` module so
    // every db test reaches for the same unique-per-invocation builder.
    // Re-export here so existing `use crate::ebook::test_support::*;`
    // globs keep resolving without touching every callsite.
    pub(crate) use crate::test_support::make_test_dir;

    /// Build an in-memory PNG of a single solid color. Used to drive
    /// `extract_accent` without baking a fixture cover into the repo.
    pub(crate) fn solid_color_png(r: u8, g: u8, b: u8, w: u32, h: u32) -> Vec<u8> {
        let img = image::RgbImage::from_pixel(w, h, image::Rgb([r, g, b]));
        let mut bytes = Vec::new();
        image::DynamicImage::ImageRgb8(img)
            .write_to(
                &mut std::io::Cursor::new(&mut bytes),
                image::ImageFormat::Png,
            )
            .expect("png encode");
        bytes
    }

    /// Path to a real fixture epub from `test_data/epubs/generated/` so
    /// cover-related tests have real OPF + embedded image bytes to work
    /// with. Stub `b"not a zip"` files won't decode.
    pub(crate) fn fixture(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("test_data")
            .join("epubs")
            .join("generated")
            .join(name)
    }

    /// Copy a fixture epub into `dest` and return the destination path.
    pub(crate) fn copy_fixture_into(name: &str, dest: &Path) -> std::path::PathBuf {
        let target = dest.join(name);
        std::fs::copy(fixture(name), &target).expect("copy fixture");
        target
    }

    /// Locate the materialized sidecar in `dir` for `<stem>` — checks both
    /// `.jpg` and `.png` since the materialized extension follows the
    /// embedded mime, which the test can't predict for arbitrary fixtures.
    pub(crate) fn find_materialized_sidecar(dir: &Path, stem: &str) -> Option<std::path::PathBuf> {
        for ext in ["jpg", "jpeg", "png"] {
            let candidate = dir.join(format!("{stem}.{ext}"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }
}
