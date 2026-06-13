//! Phase B of the ebook scan: full OPF parse for new/changed files.
//!
//! Takes the [`ParseTarget`] entries the diff in [`crate::ebook`] flagged as
//! needing fresh metadata, opens the EPUB, and produces an `IndexedBook`
//! ready for the indexer to upsert.

use std::path::{Path, PathBuf};

use epub::doc::EpubDoc;
use omnibus_shared::{Contributor, EbookMetadata, Identifier};

use super::accent::extract_accent;
use super::cover::resolve_cover;
use super::{IndexedBook, ScanOptions};

/// Phase B input: one entry per file the diff says needs a full OPF parse
/// (the New + Changed buckets). The absolute path lets the parser open the
/// file directly without re-walking; the stat values carry forward into
/// the resulting `IndexedBook` so the writer persists the same values the
/// diff observed.
#[derive(Debug, Clone)]
pub struct ParseTarget {
    pub filename: String,
    pub absolute: PathBuf,
    pub mtime_epoch: i64,
    pub size_bytes: i64,
}

/// Phase B: parse the full OPF + cover for the subset of files the diff
/// said are new or changed. Each target carries the absolute path so we
/// don't re-walk, and the Phase-A stat values so the resulting
/// `IndexedBook` ships them straight through to the writer.
pub fn parse_ebook_targets(targets: Vec<ParseTarget>, opts: ScanOptions) -> Vec<IndexedBook> {
    targets
        .into_iter()
        .map(|t| {
            let mut book = extract_metadata(&t.absolute, t.filename, &opts);
            book.mtime_epoch = t.mtime_epoch;
            book.size_bytes = t.size_bytes;
            book
        })
        .collect()
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
                // Stat values get overwritten by `parse_ebook_targets`
                // before the writer sees this struct.
                mtime_epoch: 0,
                size_bytes: 0,
            };
        }
    };

    // OPF `<dc:creator>` and `<dc:contributor>` both flow into the same
    // `books_authors_link` table at insert time (the schema does not
    // distinguish them on read). Merge them up front — creators first,
    // contributors after, in OPF source order — so downstream code only
    // sees one list. Issue #174.
    let mut creators = collect_contributors(&doc, "creator");
    creators.extend(collect_contributors(&doc, "contributor"));
    let identifiers = collect_identifiers(&doc);
    let (series, series_index) = collect_series(&doc);

    let cover = resolve_cover(path, &mut doc, opts);
    let accent = cover
        .as_ref()
        .and_then(|(_mime, bytes)| extract_accent(bytes));

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

            creators,
            subjects: all(&doc, "subject"),
            identifiers,

            series,
            series_index,
            series_id: None,

            unique_identifier: doc.unique_identifier.clone(),

            cover_url: None,
            accent,
            formats: vec![],
            added_at: None,
            error: None,
            has_override: false,
            book_files: Vec::new(),
        },
        cover,
        // Stat values get overwritten by `parse_ebook_targets` before the
        // writer sees this struct.
        mtime_epoch: 0,
        size_bytes: 0,
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
                id: None,
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
    use crate::ebook::scan_ebook_library;
    use crate::ebook::test_support::*;

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
}
