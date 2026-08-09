//! Shared publication builder: turn one book's `EbookMetadata` row into an
//! OPDS 2.0 `Publication` — the JSON counterpart to `entries::book_entry`.
//! Reuses `entries::download_link`'s format-selection decision so the Atom
//! and JSON catalogs never disagree about which format a book downloads as
//! (AC3).

use std::collections::HashSet;

use omnibus_shared::opds::{
    BelongsTo, Contributor, Link, Publication, PublicationMetadata, SeriesRef, Subject,
};
use omnibus_shared::EbookMetadata;

use super::entries::download_link;

/// Build the `Publication` for one book: metadata (title, authors,
/// description, categories, series) plus — when the book has a format this
/// catalog knows how to link to — acquisition, cover, and thumbnail links
/// (AC3).
pub(super) fn book_publication(book: &EbookMetadata) -> Publication {
    let uuid = book.unique_identifier.as_deref().filter(|u| !u.is_empty());
    let mut links = Vec::new();
    let mut images = Vec::new();
    if let Some(uuid) = uuid {
        if let Some((href, type_)) = download_link(uuid, book) {
            links.push(
                Link::new(href)
                    .with_rel("http://opds-spec.org/acquisition")
                    .with_type(type_),
            );
        }
        images.push(
            Link::new(format!("/opds/covers/{uuid}"))
                .with_rel("http://opds-spec.org/image")
                .with_type("image/jpeg"),
        );
        images.push(
            Link::new(format!("/opds/thumbs/{uuid}/sm"))
                .with_rel("http://opds-spec.org/image/thumbnail")
                .with_type("image/webp"),
        );
    }
    Publication {
        metadata: PublicationMetadata {
            schema_type: Some("http://schema.org/Book".to_string()),
            title: book.display_title(),
            identifier: uuid.map(|uuid| format!("urn:uuid:{uuid}")),
            author: book
                .creators
                .iter()
                .map(|c| Contributor {
                    name: c.name.clone(),
                })
                .collect(),
            description: book.description.clone(),
            language: book.language.clone(),
            modified: book.modified.clone(),
            published: book.published.clone(),
            subject: categories(book),
            belongs_to: series_ref(book).map(|series| BelongsTo {
                series: vec![series],
            }),
        },
        links,
        images,
    }
}

/// Combine `subjects` (scanned OPF `<dc:subject>`) and `genres`
/// (user-assigned override) into one deduplicated category list — the
/// per-publication answer to AC2's category parity, since the Atom feed
/// has no dedicated category browse to mirror.
fn categories(book: &EbookMetadata) -> Vec<Subject> {
    let mut seen = HashSet::new();
    book.subjects
        .iter()
        .chain(book.genres.iter())
        .filter(|s| seen.insert(s.as_str()))
        .map(|name| Subject { name: name.clone() })
        .collect()
}

/// `book`'s series membership as a `SeriesRef`, when it belongs to one.
/// `series_index` is a free-text OPF field (Calibre allows fractional
/// values like `"2.5"`), so a value that doesn't parse as a finite number is
/// dropped rather than surfaced as a misleading `position: 0` — `f64::parse`
/// accepts `"NaN"`/`"inf"`/`"-inf"` as valid floats, but `serde_json` cannot
/// serialize any of them, which would 500 the whole feed for one bad row.
fn series_ref(book: &EbookMetadata) -> Option<SeriesRef> {
    let name = book.series.clone()?;
    let position = book
        .series_index
        .as_deref()
        .and_then(|s| s.parse::<f64>().ok())
        .filter(|v| v.is_finite());
    Some(SeriesRef { name, position })
}
