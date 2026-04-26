//! EPUB metadata extraction (server-only).
//!
//! Walks the configured library directory, parses the OPF for each `.epub`,
//! and produces an [`IndexedBook`] per file — metadata plus the raw cover
//! bytes. Parse failures surface as `IndexedBook { metadata: EbookMetadata {
//! error: Some(_), .. }, cover: None }` so one bad file does not hide the
//! rest of the library. This output is consumed by [`crate::indexer`],
//! which writes it to the DB.
//!
//! Cover sourcing (F0.6): a `cover.{jpg,jpeg,png}` (or per-stem
//! `<basename>.{jpg,jpeg,png}`) sidecar next to the epub is preferred over
//! the embedded cover. With [`ScanOptions::materialize_sidecars`] set, the
//! scanner extracts the embedded cover into a `<basename>.jpg`/`.png`
//! sidecar on first encounter so subsequent scans skip the zip altogether.
//! Materialization is best-effort: a write failure (read-only fs,
//! permission denied) falls back to the in-memory embedded bytes for the
//! current scan and retries on the next one.

use std::path::{Path, PathBuf};

use epub::doc::{EpubDoc, EpubVersion};
use omnibus_shared::{Contributor, EbookMetadata, Identifier};

use crate::library_layout;

/// A single scanner output row — metadata plus the raw cover image bytes
/// (and mime), if the epub included one. Consumed by [`crate::queries::replace_books`].
pub struct IndexedBook {
    pub metadata: EbookMetadata,
    pub cover: Option<(String, Vec<u8>)>,
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
    /// On a successful cover extraction with no existing sidecar, write the
    /// embedded bytes to `<basename>.{jpg|png}` next to the epub. Best-effort:
    /// errors are swallowed (logged via `tracing::warn!`) so a read-only
    /// filesystem doesn't kill the scan.
    pub materialize_sidecars: bool,
}

pub fn scan_ebook_library(path: Option<&str>) -> ScanResult {
    scan_ebook_library_with(path, ScanOptions::default())
}

pub fn scan_ebook_library_with(path: Option<&str>, opts: ScanOptions) -> ScanResult {
    let Some(path_str) = path else {
        return ScanResult {
            path: None,
            books: vec![],
            error: None,
        };
    };

    let dir = Path::new(path_str);
    if !dir.exists() {
        return ScanResult {
            path: Some(path_str.to_string()),
            books: vec![],
            error: Some(format!("path not found: {path_str}")),
        };
    }

    let mut books: Vec<IndexedBook> = Vec::new();
    let mut stack: Vec<std::path::PathBuf> = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = match std::fs::read_dir(&current) {
            Ok(e) => e,
            Err(e) => {
                // Root failure is fatal (no library to surface at all). A
                // failure below the root is recorded as a synthetic entry
                // and we continue — one unreadable subfolder must not hide
                // the rest of the library, same as a single broken epub.
                if current == dir {
                    return ScanResult {
                        path: Some(path_str.to_string()),
                        books: vec![],
                        error: Some(format!("could not read directory: {e}")),
                    };
                }
                let relative = current
                    .strip_prefix(dir)
                    .unwrap_or(&current)
                    .to_string_lossy()
                    .to_string();
                books.push(IndexedBook {
                    metadata: EbookMetadata {
                        filename: relative,
                        error: Some(format!("could not read directory: {e}")),
                        ..Default::default()
                    },
                    cover: None,
                });
                continue;
            }
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let entry_path = entry.path();
            if file_type.is_dir() {
                stack.push(entry_path);
            } else if file_type.is_file()
                && entry_path
                    .extension()
                    .and_then(|s| s.to_str())
                    .map(|s| s.eq_ignore_ascii_case("epub"))
                    .unwrap_or(false)
            {
                // Use the path relative to the library root as the display
                // identifier so nested files with duplicate names don't
                // collide as component keys / error tags.
                let relative = entry_path
                    .strip_prefix(dir)
                    .unwrap_or(&entry_path)
                    .to_string_lossy()
                    .to_string();
                books.push(extract_metadata(&entry_path, relative, &opts));
            }
        }
    }

    books.sort_by(|a, b| a.metadata.filename.cmp(&b.metadata.filename));

    ScanResult {
        path: Some(path_str.to_string()),
        books,
        error: None,
    }
}

fn extract_metadata(path: &Path, filename: String, opts: &ScanOptions) -> IndexedBook {
    let mut doc = match EpubDoc::new(path) {
        Ok(d) => d,
        Err(e) => {
            return IndexedBook {
                metadata: EbookMetadata {
                    filename,
                    error: Some(format!("could not open epub: {e}")),
                    ..Default::default()
                },
                cover: None,
            };
        }
    };

    let creators = collect_contributors(&doc, "creator");
    let contributors = collect_contributors(&doc, "contributor");
    let identifiers = collect_identifiers(&doc);
    let (series, series_index) = collect_series(&doc);

    let cover = resolve_cover(path, &mut doc, opts);

    IndexedBook {
        metadata: EbookMetadata {
            id: 0,
            filename,
            title: first(&doc, "title"),
            description: first(&doc, "description"),
            publisher: first(&doc, "publisher"),
            published: first(&doc, "date"),
            modified: first(&doc, "dcterms:modified"),
            language: first(&doc, "language"),
            rights: first(&doc, "rights"),
            source: first(&doc, "source"),
            coverage: first(&doc, "coverage"),
            dc_type: first(&doc, "type"),
            dc_format: first(&doc, "format"),
            relation: first(&doc, "relation"),

            creators,
            contributors,
            subjects: all(&doc, "subject"),
            identifiers,

            series,
            series_index,

            epub_version: Some(format_version(doc.version)),
            unique_identifier: doc.unique_identifier.clone(),
            resource_count: doc.resources.len(),
            spine_count: doc.spine.len(),
            toc_count: doc.toc.len(),

            cover_url: None,
            error: None,
        },
        cover,
    }
}

/// Sidecar-first cover resolution.
///
/// 1. If `<path>` has a sidecar (per-stem first, folder-level fallback),
///    read its bytes and return them.
/// 2. Otherwise, ask the EPUB for its embedded cover.
/// 3. If `opts.materialize_sidecars` is set and the embedded cover came back
///    successfully, write it to a `<basename>.{jpg|png}` sidecar so the next
///    scan hits the sidecar directly. Failures are non-fatal.
///
/// Returns the cover bytes used for *this* scan (the in-memory copy, even
/// when materialization wrote them to disk — this avoids a round-trip read).
fn resolve_cover<R: std::io::Read + std::io::Seek>(
    path: &Path,
    doc: &mut EpubDoc<R>,
    opts: &ScanOptions,
) -> Option<(String, Vec<u8>)> {
    let mut corrupt_sidecar: Option<PathBuf> = None;
    if let Some(sidecar) = library_layout::sidecar_cover_for(path) {
        if let Some(bytes) = read_sidecar(&sidecar) {
            return Some(bytes);
        }
        // Sidecar lookup found a file but reading it failed — fall through
        // to the embedded path. Pass the broken path to materialize_sidecar
        // so it can repair the cache instead of refusing forever.
        corrupt_sidecar = Some(sidecar);
    }

    let embedded = doc.get_cover().map(|(bytes, mime)| {
        let mime = if mime.is_empty() {
            "image/jpeg".to_string()
        } else {
            mime
        };
        (mime, bytes)
    });

    if opts.materialize_sidecars {
        if let Some((mime, bytes)) = embedded.as_ref() {
            materialize_sidecar(path, mime, bytes, corrupt_sidecar.as_deref());
        }
    }

    embedded
}

fn read_sidecar(path: &Path) -> Option<(String, Vec<u8>)> {
    let bytes = std::fs::read(path).ok()?;
    // A zero-length file is no better than a read error — surfacing it as
    // "the cover" would just blank the row in the UI. Treat as corrupt so
    // the materialize path can repair it next pass.
    if bytes.is_empty() {
        return None;
    }
    let mime = mime_for_extension(path).to_string();
    Some((mime, bytes))
}

fn mime_for_extension(path: &Path) -> &'static str {
    match path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .as_deref()
    {
        Some("png") => "image/png",
        // jpg / jpeg / anything else falls back to JPEG. Embedded EPUB covers
        // are overwhelmingly JPEG, and a wrong-but-close mime is better than
        // none for the cover endpoint.
        _ => "image/jpeg",
    }
}

/// Best-effort write of `<basename>.{jpg|png}` next to the epub so future
/// scans skip the zip. Fails silently — this is a cache, not a contract.
///
/// `corrupt_sidecar`, when set, is a sidecar path the caller already
/// confirmed is unreadable. We allow overwriting *exactly* that file so a
/// corrupt cache entry self-heals on the next scan instead of forcing every
/// future scan to re-open the zip. Anything else under `target.exists()`
/// (a valid file, a different filename) we leave alone.
fn materialize_sidecar(epub_path: &Path, mime: &str, bytes: &[u8], corrupt_sidecar: Option<&Path>) {
    let Some(parent) = epub_path.parent() else {
        return;
    };
    let Some(stem) = epub_path.file_stem().and_then(|s| s.to_str()) else {
        return;
    };
    let ext = if mime.eq_ignore_ascii_case("image/png") {
        "png"
    } else {
        "jpg"
    };
    let target = parent.join(format!("{stem}.{ext}"));
    if target.exists() {
        let is_known_corrupt = corrupt_sidecar.is_some_and(|p| p == target.as_path());
        if !is_known_corrupt {
            // A valid file we don't own (race or user-dropped sidecar). Don't
            // clobber.
            return;
        }
        // Fall through and overwrite — std::fs::write truncates the existing
        // file, repairing the cache.
        tracing::warn!(
            path = %target.display(),
            "repairing unreadable cover sidecar"
        );
    }
    if let Err(e) = std::fs::write(&target, bytes) {
        tracing::warn!(
            error = %e,
            path = %target.display(),
            "could not materialize cover sidecar; falling back to embedded for this scan"
        );
    }
}

fn format_version(v: EpubVersion) -> String {
    match v {
        EpubVersion::Version2_0 => "2.0".to_string(),
        EpubVersion::Version3_0 => "3.0".to_string(),
        other => format!("{other:?}"),
    }
}

fn collect_contributors<R: std::io::Read + std::io::Seek>(
    doc: &EpubDoc<R>,
    key: &str,
) -> Vec<Contributor> {
    doc.metadata
        .iter()
        .filter(|m| m.property == key)
        .filter_map(|m| {
            let name = m.value.trim().to_string();
            if name.is_empty() {
                return None;
            }
            let role = m
                .refinement("role")
                .map(|r| r.value.clone())
                .or_else(|| lookup_refinement(&m.refined, "role"));
            let file_as = m
                .refinement("file-as")
                .map(|r| r.value.clone())
                .or_else(|| lookup_refinement(&m.refined, "file-as"));
            Some(Contributor {
                name,
                role,
                file_as,
            })
        })
        .collect()
}

fn lookup_refinement(refs: &[epub::doc::MetadataRefinement], key: &str) -> Option<String> {
    refs.iter()
        .find(|r| r.property == key)
        .map(|r| r.value.clone())
}

/// Resolve (series, series_index) from the OPF.
///
/// EPUB3 stores a series as a `belongs-to-collection` metadata entry whose
/// `group-position` refinement holds the index. Calibre's legacy EPUB2 tooling
/// writes top-level `<meta name="calibre:series">` and `calibre:series_index`
/// entries instead. We try EPUB3 first (with the refinement), then fall back
/// to the Calibre keys.
fn collect_series<R: std::io::Read + std::io::Seek>(
    doc: &EpubDoc<R>,
) -> (Option<String>, Option<String>) {
    if let Some(m) = doc
        .metadata
        .iter()
        .find(|m| m.property == "belongs-to-collection")
    {
        let name = m.value.trim().to_string();
        if !name.is_empty() {
            let idx = lookup_refinement(&m.refined, "group-position")
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty());
            return (Some(name), idx);
        }
    }
    (
        first(doc, "calibre:series"),
        first(doc, "calibre:series_index"),
    )
}

fn collect_identifiers<R: std::io::Read + std::io::Seek>(doc: &EpubDoc<R>) -> Vec<Identifier> {
    doc.metadata
        .iter()
        .filter(|m| m.property == "identifier")
        .filter_map(|m| {
            let value = m.value.trim().to_string();
            if value.is_empty() {
                return None;
            }
            let scheme = lookup_refinement(&m.refined, "scheme")
                .or_else(|| lookup_refinement(&m.refined, "identifier-type"));
            Some(Identifier { value, scheme })
        })
        .collect()
}

fn first<R: std::io::Read + std::io::Seek>(doc: &EpubDoc<R>, key: &str) -> Option<String> {
    doc.metadata
        .iter()
        .find(|m| m.property == key)
        .map(|m| m.value.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn all<R: std::io::Read + std::io::Seek>(doc: &EpubDoc<R>, key: &str) -> Vec<String> {
    doc.metadata
        .iter()
        .filter(|m| m.property == key)
        .map(|m| m.value.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// Build a unique temp directory per test invocation. Rust runs unit
    /// tests in parallel by default, so a fixed path under `temp_dir()`
    /// would collide between tests (and between repeated runs).
    fn make_test_dir(suffix: &str) -> std::path::PathBuf {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let pid = std::process::id();
        let seq = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir = std::env::temp_dir().join(format!("omnibus_ebook_{suffix}_{pid}_{seq}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("should create test dir");
        dir
    }

    #[test]
    fn scan_with_no_path_returns_empty() {
        let out = scan_ebook_library(None);
        assert!(out.books.is_empty());
        assert!(out.path.is_none());
    }

    #[test]
    fn scan_with_missing_path_reports_error() {
        let out = scan_ebook_library(Some("/definitely/does/not/exist/omnibus_ebook_test"));
        assert!(out.error.is_some());
    }

    #[test]
    fn scan_ignores_non_epub_files() {
        let dir = make_test_dir("ignore");
        std::fs::write(dir.join("notes.txt"), b"hi").unwrap();
        let out = scan_ebook_library(Some(dir.to_str().unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(out.books.is_empty());
        assert!(out.error.is_none());
    }

    #[test]
    fn scan_recurses_into_subdirectories() {
        let dir = make_test_dir("recursive");
        let nested = dir.join("series").join("vol1");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(dir.join("top.epub"), b"not a zip").unwrap();
        std::fs::write(nested.join("deep.epub"), b"not a zip").unwrap();
        std::fs::write(nested.join("cover.jpg"), b"ignore").unwrap();
        let out = scan_ebook_library(Some(dir.to_str().unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
        // Two epubs + zero synthetic dir errors under a healthy tree.
        let book_entries: Vec<&str> = out
            .books
            .iter()
            .filter(|b| b.metadata.error.is_none() || !b.metadata.filename.is_empty())
            .map(|b| b.metadata.filename.as_str())
            .collect();
        assert_eq!(out.books.len(), 2);
        assert!(book_entries.contains(&"top.epub"));
        assert!(book_entries
            .iter()
            .any(|n| n.ends_with("deep.epub") && n.contains("series")));
    }

    #[test]
    fn scan_records_parse_errors_per_file() {
        let dir = make_test_dir("bad");
        std::fs::write(dir.join("broken.epub"), b"not actually a zip").unwrap();
        let out = scan_ebook_library(Some(dir.to_str().unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(out.books.len(), 1);
        assert!(out.books[0].metadata.error.is_some());
        assert_eq!(out.books[0].metadata.filename, "broken.epub");
    }

    // Permission tests only run where we can reliably strip read access
    // from a directory — on Windows, even removing all permission bits
    // doesn't reliably make `read_dir` fail as the owning process.
    #[cfg(unix)]
    #[test]
    fn scan_continues_past_unreadable_subdirectory() {
        use std::os::unix::fs::PermissionsExt;

        let dir = make_test_dir("unreadable_subdir");
        std::fs::write(dir.join("good.epub"), b"not a zip").unwrap();
        let locked = dir.join("locked");
        std::fs::create_dir_all(&locked).unwrap();
        // Drop all permissions so read_dir on the subdir fails. Skip the
        // test on platforms where this doesn't take effect (e.g. running
        // as root in CI containers) rather than asserting a false
        // positive.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o000)).unwrap();
        if std::fs::read_dir(&locked).is_ok() {
            std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::remove_dir_all(&dir).unwrap();
            return;
        }

        let out = scan_ebook_library(Some(dir.to_str().unwrap()));

        // Restore permissions before removing the tree, otherwise cleanup
        // fails on some platforms.
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        // Top-level scan must not report a fatal error.
        assert!(out.error.is_none());
        // Good epub still surfaces.
        assert!(out.books.iter().any(|b| b.metadata.filename == "good.epub"));
        // Locked subdir surfaces as a synthetic error entry, not silently
        // dropped.
        assert!(out
            .books
            .iter()
            .any(|b| b.metadata.filename == "locked" && b.metadata.error.is_some()));
    }

    // ---------- Sidecar cover (F0.6) ----------

    /// Path to a real fixture epub from `test_data/epubs/generated/` so
    /// cover-related tests have real OPF + embedded image bytes to work
    /// with. Stub `b"not a zip"` files won't decode.
    fn fixture(name: &str) -> std::path::PathBuf {
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join("test_data")
            .join("epubs")
            .join("generated")
            .join(name)
    }

    /// Copy a fixture epub into `dest` and return the destination path.
    fn copy_fixture_into(name: &str, dest: &Path) -> std::path::PathBuf {
        let target = dest.join(name);
        std::fs::copy(fixture(name), &target).expect("copy fixture");
        target
    }

    #[test]
    fn extract_metadata_uses_sidecar_when_present() {
        // alpha.epub ships an embedded cover. Plant a recognizably-different
        // sidecar next to it; the scanner must return the sidecar bytes.
        let dir = make_test_dir("sidecar_wins");
        copy_fixture_into("alpha.epub", &dir);
        let sidecar_bytes: &[u8] = b"sidecar-jpg-magic-bytes";
        std::fs::write(dir.join("alpha.jpg"), sidecar_bytes).unwrap();

        let out = scan_ebook_library(Some(dir.to_str().unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();

        let alpha = out
            .books
            .iter()
            .find(|b| b.metadata.filename == "alpha.epub")
            .expect("alpha present");
        let (mime, bytes) = alpha.cover.as_ref().expect("cover present");
        assert_eq!(bytes, sidecar_bytes, "expected sidecar bytes, got embedded");
        assert_eq!(mime, "image/jpeg");
    }

    #[test]
    fn extract_metadata_uses_embedded_when_no_sidecar() {
        // alpha.epub has an embedded cover; no sidecar planted. We don't
        // know the exact embedded bytes, but they should be non-empty and
        // the cover slot must be populated. Default ScanOptions disables
        // materialization, so no sidecar should appear after the scan.
        let dir = make_test_dir("embedded_only");
        copy_fixture_into("alpha.epub", &dir);

        let out = scan_ebook_library(Some(dir.to_str().unwrap()));
        let sidecar_appeared = find_materialized_sidecar(&dir, "alpha").is_some();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(
            !sidecar_appeared,
            "default ScanOptions must not materialize sidecars"
        );
        let alpha = out
            .books
            .iter()
            .find(|b| b.metadata.filename == "alpha.epub")
            .expect("alpha present");
        let (_, bytes) = alpha.cover.as_ref().expect("embedded cover present");
        assert!(!bytes.is_empty());
    }

    /// Locate the materialized sidecar in `dir` for `<stem>` — checks both
    /// `.jpg` and `.png` since the materialized extension follows the
    /// embedded mime, which the test can't predict for arbitrary fixtures.
    fn find_materialized_sidecar(dir: &Path, stem: &str) -> Option<std::path::PathBuf> {
        for ext in ["jpg", "jpeg", "png"] {
            let candidate = dir.join(format!("{stem}.{ext}"));
            if candidate.exists() {
                return Some(candidate);
            }
        }
        None
    }

    #[test]
    fn extract_metadata_materializes_sidecar_with_opt_in() {
        // With `materialize_sidecars: true`, scanning an epub that has an
        // embedded cover but no sidecar must write `<basename>.{jpg|png}`
        // (extension matches embedded mime) next to the file so subsequent
        // scans hit the sidecar directly.
        let dir = make_test_dir("materialize");
        copy_fixture_into("alpha.epub", &dir);
        assert!(
            find_materialized_sidecar(&dir, "alpha").is_none(),
            "precondition: no sidecar yet"
        );

        let out = scan_ebook_library_with(
            Some(dir.to_str().unwrap()),
            ScanOptions {
                materialize_sidecars: true,
            },
        );

        let sidecar = find_materialized_sidecar(&dir, "alpha");
        let written = sidecar.as_ref().and_then(|p| std::fs::read(p).ok());
        let alpha = out
            .books
            .iter()
            .find(|b| b.metadata.filename == "alpha.epub")
            .map(|b| b.cover.as_ref().map(|(_, bytes)| bytes.clone()))
            .unwrap_or(None);
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(sidecar.is_some(), "sidecar should have been written");
        assert_eq!(
            written.as_deref(),
            alpha.as_deref(),
            "written sidecar bytes must match returned cover bytes"
        );
    }

    #[test]
    fn extract_metadata_second_scan_reads_sidecar_not_zip() {
        // After materialization, swap the sidecar with different bytes. The
        // next scan should return *those* bytes, proving the read came from
        // the sidecar and not the unchanged embedded cover in the zip.
        let dir = make_test_dir("second_scan");
        copy_fixture_into("alpha.epub", &dir);

        // First scan: materialize.
        let _ = scan_ebook_library_with(
            Some(dir.to_str().unwrap()),
            ScanOptions {
                materialize_sidecars: true,
            },
        );

        let sidecar_path =
            find_materialized_sidecar(&dir, "alpha").expect("first scan materialized a sidecar");

        // Replace the sidecar (same path/extension) with sentinel bytes.
        let sentinel: &[u8] = b"replaced-after-materialization";
        std::fs::write(&sidecar_path, sentinel).unwrap();

        // Second scan (default opts) — should read the sentinel, not
        // re-extract the embedded cover.
        let out = scan_ebook_library(Some(dir.to_str().unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();

        let alpha = out
            .books
            .iter()
            .find(|b| b.metadata.filename == "alpha.epub")
            .expect("alpha present");
        let (_, bytes) = alpha.cover.as_ref().expect("cover present");
        assert_eq!(
            bytes, sentinel,
            "second scan should have read the swapped sidecar"
        );
    }

    #[test]
    fn extract_metadata_repairs_unreadable_sidecar_on_materialize() {
        // alpha.epub has an embedded cover. Plant a zero-length sidecar that
        // sidecar_cover_for() will pick up but read_sidecar() can't use.
        // With materialize_sidecars=true, the broken cache must be repaired
        // so the next scan reads the sidecar instead of re-opening the zip.
        let dir = make_test_dir("repair_sidecar");
        copy_fixture_into("alpha.epub", &dir);

        // alpha.epub embeds a PNG, so the materializer would write
        // `alpha.png`. Plant the corrupt sidecar at that exact path.
        let broken = dir.join("alpha.png");
        std::fs::write(&broken, b"").unwrap();

        let out = scan_ebook_library_with(
            Some(dir.to_str().unwrap()),
            ScanOptions {
                materialize_sidecars: true,
            },
        );

        let repaired_bytes = std::fs::read(&broken).expect("sidecar still on disk");
        let alpha_cover = out
            .books
            .iter()
            .find(|b| b.metadata.filename == "alpha.epub")
            .and_then(|b| b.cover.as_ref().map(|(_, bytes)| bytes.clone()));
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(
            !repaired_bytes.is_empty(),
            "broken zero-length sidecar should have been repaired"
        );
        assert_eq!(
            alpha_cover.as_deref(),
            Some(repaired_bytes.as_slice()),
            "repaired sidecar bytes must match the embedded cover the scan returned"
        );
    }

    #[test]
    fn extract_metadata_does_not_clobber_unrelated_existing_sidecar() {
        // The repair gate must only overwrite the *exact* corrupt file the
        // sidecar lookup returned — never a different valid file that
        // happens to sit at the materialize target.
        //
        // Setup: alpha.epub embeds a PNG, so materialize would target
        // alpha.png. We plant a corrupt (empty) `alpha.jpg` (which jpg-over-
        // png priority makes sidecar_cover_for return) AND a valid `alpha.png`
        // (the user's curated cover). The materialize step must refuse to
        // overwrite alpha.png because the *known* corrupt path is alpha.jpg.
        let dir = make_test_dir("no_clobber");
        copy_fixture_into("alpha.epub", &dir);

        let corrupt_jpg = dir.join("alpha.jpg");
        std::fs::write(&corrupt_jpg, b"").unwrap();
        let valid_png = dir.join("alpha.png");
        let curated: &[u8] = b"user-curated-cover-do-not-touch";
        std::fs::write(&valid_png, curated).unwrap();

        let _ = scan_ebook_library_with(
            Some(dir.to_str().unwrap()),
            ScanOptions {
                materialize_sidecars: true,
            },
        );

        let png_after = std::fs::read(&valid_png).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        assert_eq!(
            png_after, curated,
            "alpha.png is not the corrupt sidecar — must not be overwritten"
        );
    }

    #[test]
    fn extract_metadata_no_embedded_no_sidecar_returns_none() {
        // gamma.epub has no embedded cover. No sidecar planted, no
        // materialization. Cover should stay None and no file should be
        // written.
        let dir = make_test_dir("no_cover");
        copy_fixture_into("gamma.epub", &dir);

        let out = scan_ebook_library_with(
            Some(dir.to_str().unwrap()),
            ScanOptions {
                materialize_sidecars: true,
            },
        );
        let sidecar_appeared = find_materialized_sidecar(&dir, "gamma").is_some();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(
            !sidecar_appeared,
            "no embedded cover → nothing to materialize"
        );
        let gamma = out
            .books
            .iter()
            .find(|b| b.metadata.filename == "gamma.epub")
            .expect("gamma present");
        assert!(gamma.cover.is_none());
    }

    #[test]
    fn scan_handles_calibre_shaped_tree_and_ignores_metadata_opf() {
        // Lock in the read-tolerance promise from F0.6: a Calibre-style
        // library tree (`<Lastname, First>/Title (id)/title.epub` plus an
        // adjacent `metadata.opf` Calibre wrote out) must scan correctly.
        // We assert (a) the epub is found, (b) the title comes from the
        // *embedded* OPF inside the epub, not the deliberately-wrong
        // sidecar `metadata.opf`. The sidecar is ignored entirely.
        let dir = make_test_dir("calibre_shaped");
        let book_dir = dir.join("Lovelace, Ada").join("Alpha (42)");
        std::fs::create_dir_all(&book_dir).unwrap();
        std::fs::copy(fixture("alpha.epub"), book_dir.join("alpha.epub")).unwrap();
        // Calibre's metadata.opf — write garbage into it so any code path
        // that *did* read it would visibly disagree with the embedded OPF.
        std::fs::write(
            book_dir.join("metadata.opf"),
            br#"<?xml version="1.0"?>
<package xmlns="http://www.idpf.org/2007/opf" version="2.0">
<metadata xmlns:dc="http://purl.org/dc/elements/1.1/">
<dc:title>WRONG TITLE FROM CALIBRE SIDECAR</dc:title>
<dc:creator>Wrong Author</dc:creator>
</metadata></package>"#,
        )
        .unwrap();

        let out = scan_ebook_library(Some(dir.to_str().unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(out.error.is_none(), "scan errored: {:?}", out.error);
        // Exactly one epub found despite the metadata.opf sibling.
        let epubs: Vec<_> = out
            .books
            .iter()
            .filter(|b| b.metadata.error.is_none())
            .collect();
        assert_eq!(epubs.len(), 1);
        // Title comes from the embedded OPF, not the misleading sidecar.
        assert_eq!(epubs[0].metadata.title.as_deref(), Some("Alpha"));
    }

    #[cfg(unix)]
    #[test]
    fn extract_metadata_materialization_failure_falls_back_to_embedded() {
        // chmod the directory read-only-execute so write fails. The scanner
        // must still return cover bytes (from embedded), and no sidecar
        // should appear.
        use std::os::unix::fs::PermissionsExt;

        let dir = make_test_dir("readonly_dir");
        copy_fixture_into("alpha.epub", &dir);
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o555)).unwrap();

        // Skip if the chmod didn't take (e.g. running as root in some CI
        // containers).
        if std::fs::write(dir.join("write_probe"), b"x").is_ok() {
            std::fs::remove_file(dir.join("write_probe")).ok();
            std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
            std::fs::remove_dir_all(&dir).unwrap();
            return;
        }

        let out = scan_ebook_library_with(
            Some(dir.to_str().unwrap()),
            ScanOptions {
                materialize_sidecars: true,
            },
        );

        let sidecar_appeared = find_materialized_sidecar(&dir, "alpha").is_some();

        // Restore perms before cleanup.
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        std::fs::remove_dir_all(&dir).unwrap();

        assert!(!sidecar_appeared, "read-only fs must not produce a sidecar");
        let alpha = out
            .books
            .iter()
            .find(|b| b.metadata.filename == "alpha.epub")
            .expect("alpha present");
        let (_, bytes) = alpha.cover.as_ref().expect("embedded fallback present");
        assert!(!bytes.is_empty(), "embedded fallback must be non-empty");
    }
}
