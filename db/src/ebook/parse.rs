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
                word_count: None,
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

    // Estimate the word count while the EPUB is already open, so
    // `books.word_count` is populated at index time and the stats Pages tile
    // never has to reopen the file. Walks the spine text — heavier than the
    // OPF read above, but paid only for new/changed files, once each.
    let word_count = super::estimate_word_count(&mut doc);

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
            // Derived at read time from `book_identifiers` (`row_to_ebook`
            // -> `derive_isbn13`), not at parse time — this struct is
            // written into the normalized tables before that derivation
            // ever runs.
            isbn13: None,

            series,
            series_index,
            series_id: None,

            unique_identifier: doc.unique_identifier.clone(),

            cover_url: None,
            accent,
            formats: vec![],
            // Derived at read time from `physical_copies` (projection), not at
            // parse time — this struct is written before that derivation runs.
            has_physical: false,
            added_at: None,
            error: None,
            has_override: false,
            has_cover_override: false,
            book_files: Vec::new(),
            epub_size_bytes: None,
            // Both resolved in `get_book`, which is the only read that
            // knows the book's `book_files` rows.
            epub_validator: None,
            audio_validator: None,
        },
        cover,
        // Stat values get overwritten by `parse_ebook_targets` before the
        // writer sees this struct.
        mtime_epoch: 0,
        size_bytes: 0,
        word_count,
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
mod tests;
