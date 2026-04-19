//! EPUB metadata extraction (server-only).
//!
//! Opens each `.epub` under the configured ebook library path, pulls the
//! Dublin Core metadata plus the embedded cover image, and returns a
//! serialisable `EbookMetadata` per file. Parse failures surface as
//! `EbookMetadata { error: Some(_), .. }` so one bad file does not hide the
//! rest of the library.

use std::path::Path;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use epub::doc::EpubDoc;
use omnibus_shared::{EbookLibrary, EbookMetadata};

pub fn scan_ebook_library(path: Option<&str>) -> EbookLibrary {
    let Some(path_str) = path else {
        return EbookLibrary::default();
    };

    let dir = Path::new(path_str);
    if !dir.exists() {
        return EbookLibrary {
            path: Some(path_str.to_string()),
            books: vec![],
            error: Some(format!("path not found: {path_str}")),
        };
    }

    let entries = match std::fs::read_dir(dir) {
        Ok(e) => e,
        Err(e) => {
            return EbookLibrary {
                path: Some(path_str.to_string()),
                books: vec![],
                error: Some(format!("could not read directory: {e}")),
            };
        }
    };

    let mut books: Vec<EbookMetadata> = entries
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().map(|t| t.is_file()).unwrap_or(false))
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|s| s.to_str())
                .map(|s| s.eq_ignore_ascii_case("epub"))
                .unwrap_or(false)
        })
        .map(|e| {
            let filename = e.file_name().to_string_lossy().to_string();
            extract_metadata(&e.path(), filename)
        })
        .collect();

    books.sort_by(|a, b| a.filename.cmp(&b.filename));

    EbookLibrary {
        path: Some(path_str.to_string()),
        books,
        error: None,
    }
}

fn extract_metadata(path: &Path, filename: String) -> EbookMetadata {
    let mut doc = match EpubDoc::new(path) {
        Ok(d) => d,
        Err(e) => {
            return EbookMetadata {
                filename,
                error: Some(format!("could not open epub: {e}")),
                ..Default::default()
            };
        }
    };

    let title = first(&doc, "title");
    let authors = all(&doc, "creator");
    let description = first(&doc, "description");
    let publisher = first(&doc, "publisher");
    let published = first(&doc, "date");
    let language = first(&doc, "language");
    let identifier = first(&doc, "identifier");
    let subjects = all(&doc, "subject");
    // Calibre stores series as legacy <meta name="calibre:series"> which the
    // epub crate surfaces under the raw name. EPUB3 uses
    // `belongs-to-collection`; try both.
    let series = first(&doc, "calibre:series").or_else(|| first(&doc, "belongs-to-collection"));
    let series_index =
        first(&doc, "calibre:series_index").or_else(|| first(&doc, "group-position"));

    let cover_image = doc.get_cover().map(|(bytes, mime)| {
        let mime = if mime.is_empty() {
            "image/jpeg".to_string()
        } else {
            mime
        };
        format!("data:{};base64,{}", mime, STANDARD.encode(&bytes))
    });

    EbookMetadata {
        filename,
        title,
        authors,
        description,
        publisher,
        published,
        language,
        identifier,
        subjects,
        series,
        series_index,
        cover_image,
        error: None,
    }
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
        let dir = std::env::temp_dir().join("omnibus_ebook_scan_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.txt"), b"hi").unwrap();
        let out = scan_ebook_library(Some(dir.to_str().unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
        assert!(out.books.is_empty());
        assert!(out.error.is_none());
    }

    #[test]
    fn scan_records_parse_errors_per_file() {
        let dir = std::env::temp_dir().join("omnibus_ebook_bad_test");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("broken.epub"), b"not actually a zip").unwrap();
        let out = scan_ebook_library(Some(dir.to_str().unwrap()));
        std::fs::remove_dir_all(&dir).unwrap();
        assert_eq!(out.books.len(), 1);
        assert!(out.books[0].error.is_some());
        assert_eq!(out.books[0].filename, "broken.epub");
    }
}
