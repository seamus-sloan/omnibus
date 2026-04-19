//! EPUB metadata extraction (server-only).
//!
//! Opens each `.epub` under the configured ebook library path, pulls the
//! Dublin Core metadata plus the embedded cover image, and returns a
//! serialisable `EbookMetadata` per file. Parse failures surface as
//! `EbookMetadata { error: Some(_), .. }` so one bad file does not hide the
//! rest of the library.
//!
//! The OPF metadata schema is open-ended — publishers, Calibre, Sigil and
//! DRM toolchains all stuff custom `<meta>` elements into the package
//! document. We extract the well-known Dublin Core fields into typed slots
//! and pass *every* entry through in `raw_metadata` so the UI can render the
//! full picture.

use std::path::Path;

use base64::{engine::general_purpose::STANDARD, Engine as _};
use epub::doc::{EpubDoc, EpubVersion};
use omnibus_shared::{Contributor, EbookLibrary, EbookMetadata, Identifier, RawMeta};

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

    let creators = collect_contributors(&doc, "creator");
    let contributors = collect_contributors(&doc, "contributor");
    let identifiers = collect_identifiers(&doc);

    let cover_image = doc.get_cover().map(|(bytes, mime)| {
        let mime = if mime.is_empty() {
            "image/jpeg".to_string()
        } else {
            mime
        };
        format!("data:{};base64,{}", mime, STANDARD.encode(&bytes))
    });

    let raw_metadata = doc
        .metadata
        .iter()
        .map(|m| RawMeta {
            property: m.property.clone(),
            value: m.value.clone(),
            lang: m.lang.clone(),
            refinements: m
                .refined
                .iter()
                .map(|r| (r.property.clone(), r.value.clone()))
                .collect(),
        })
        .collect();

    EbookMetadata {
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

        // Calibre stores series via legacy `<meta name="calibre:series">`;
        // EPUB3 uses `belongs-to-collection` with a `group-position` refinement.
        series: first(&doc, "calibre:series").or_else(|| first(&doc, "belongs-to-collection")),
        series_index: first(&doc, "calibre:series_index").or_else(|| first(&doc, "group-position")),

        epub_version: Some(format_version(doc.version)),
        unique_identifier: doc.unique_identifier.clone(),
        resource_count: doc.resources.len(),
        spine_count: doc.spine.len(),
        toc_count: doc.toc.len(),

        cover_image,
        raw_metadata,
        error: None,
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
