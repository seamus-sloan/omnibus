//! EPUB metadata extraction (server-only).
//!
//! Walks the configured library directory, parses the OPF for each `.epub`,
//! and produces an [`IndexedBook`] per file — metadata plus the raw cover
//! bytes. Parse failures surface as `IndexedBook { metadata: EbookMetadata {
//! error: Some(_), .. }, cover: None }` so one bad file does not hide the
//! rest of the library. This output is consumed by [`crate::indexer`],
//! which writes it to the DB.

use std::path::Path;

use epub::doc::{EpubDoc, EpubVersion};
use omnibus_shared::{Contributor, EbookMetadata, Identifier};

/// A single scanner output row — metadata plus the raw cover image bytes
/// (and mime), if the epub included one. Consumed by [`crate::db::replace_books`].
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

pub fn scan_ebook_library(path: Option<&str>) -> ScanResult {
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
                books.push(extract_metadata(&entry_path, relative));
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

fn extract_metadata(path: &Path, filename: String) -> IndexedBook {
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

    let cover = doc.get_cover().map(|(bytes, mime)| {
        let mime = if mime.is_empty() {
            "image/jpeg".to_string()
        } else {
            mime
        };
        (mime, bytes)
    });

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
}
