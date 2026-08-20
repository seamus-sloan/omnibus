//! The copyable fields, as data.
//!
//! This module is the point of the compare view. The panel it replaced grew a
//! diff row, a selection signal, and an apply branch **per field**, in three
//! separate places — three edits for every new field, and silently wrong if
//! one was missed. Here a field is one [`MetadataField`] variant plus its arms
//! in four exhaustive `match`es, and the compare view iterates
//! [`MetadataField::ALL`]: no row is written by hand, so none can be forgotten.

use dioxus::prelude::*;
use omnibus_shared::metadata_lookup::ProviderEdition;

use super::super::form_grid::FormFields;

/// One field a source can offer and the form can take.
///
/// Adding a field is: a variant here, its entry in [`Self::ALL`], and the arms
/// the compiler then demands in [`Self::label`], [`Self::current`],
/// [`Self::source_value`], and [`Self::apply`]. Nothing in the compare view
/// itself changes — it renders [`Self::ALL`] and nothing else.
///
/// Four of those five steps are compiler-enforced: each accessor is an
/// exhaustive `match`, so a new variant fails to build until every one has an
/// arm. Adding it to [`Self::ALL`] is the single manual step, which is why
/// the completeness test drives every entry through all four accessors rather
/// than only reading its label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MetadataField {
    Title,
    Authors,
    Publisher,
    Published,
    Series,
    Isbn13,
    Isbn10,
    PrintPages,
    Genres,
    Description,
}

impl MetadataField {
    /// Every copyable field, in the order the compare view renders them:
    /// identity first, then the long free text, mirroring the edit form's own
    /// running order so the two read as the same book.
    ///
    /// **Tags are deliberately absent.** A provider's subject list lands on
    /// `genres` in this codebase (#1659); `subjects` is whatever the EPUB's
    /// own `<dc:subject>` entries were, and overwriting that with a provider's
    /// vocabulary would destroy the one field that describes *this file*.
    /// Series position is absent for a plainer reason: no provider carries it.
    pub(super) const ALL: &'static [MetadataField] = &[
        MetadataField::Title,
        MetadataField::Authors,
        MetadataField::Publisher,
        MetadataField::Published,
        MetadataField::Series,
        MetadataField::Isbn13,
        MetadataField::Isbn10,
        MetadataField::PrintPages,
        MetadataField::Genres,
        MetadataField::Description,
    ];

    /// The row heading, matching the edit form's own label for the same field
    /// so a reader can see where a copied value will land.
    pub(super) fn label(self) -> &'static str {
        match self {
            MetadataField::Title => "Title",
            MetadataField::Authors => "Author(s)",
            MetadataField::Publisher => "Publisher",
            MetadataField::Published => "Published",
            MetadataField::Series => "Series",
            MetadataField::Isbn13 => "ISBN-13",
            MetadataField::Isbn10 => "ISBN-10",
            MetadataField::PrintPages => "Print Pages",
            MetadataField::Genres => "Genres",
            MetadataField::Description => "Description",
        }
    }

    /// What the *form* currently holds — the staged value, not the saved one,
    /// so a row reflects an apply the moment it happens.
    pub(super) fn current(self, fields: FormFields) -> String {
        match self {
            MetadataField::Title => fields.title.read().clone(),
            MetadataField::Authors => fields.authors.read().join(", "),
            MetadataField::Publisher => fields.publisher.read().clone(),
            MetadataField::Published => fields.published.read().clone(),
            MetadataField::Series => fields.series.read().clone(),
            MetadataField::Isbn13 => fields.isbn13.read().clone(),
            MetadataField::Isbn10 => fields.isbn10.read().clone(),
            MetadataField::PrintPages => fields.print_pages.read().clone(),
            MetadataField::Genres => fields.genres.read().join(", "),
            MetadataField::Description => fields.description.read().clone(),
        }
    }

    /// What the source offers, or the empty string when it offers nothing.
    ///
    /// The empty string is the whole safety property of this screen: it is
    /// what [`Self::is_available`] reads, and an unavailable field can't be
    /// applied — so a provider that doesn't know a field can never blank out
    /// a value the reader already has.
    pub(super) fn source_value(self, edition: &ProviderEdition) -> String {
        let text = |v: &Option<String>| v.as_deref().unwrap_or_default().trim().to_string();
        // Blank entries are dropped before joining, so a list of them reads
        // as absent rather than as the separator string — `["", ""]` would
        // otherwise render and stage as ", ".
        let list = |v: &[String]| {
            v.iter()
                .map(|s| s.trim())
                .filter(|s| !s.is_empty())
                .collect::<Vec<_>>()
                .join(", ")
        };
        match self {
            MetadataField::Title => edition.title.trim().to_string(),
            MetadataField::Authors => list(&edition.authors),
            MetadataField::Publisher => text(&edition.publisher),
            MetadataField::Published => text(&edition.year),
            MetadataField::Series => text(&edition.series),
            MetadataField::Isbn13 => edition.isbn13.trim().to_string(),
            MetadataField::Isbn10 => text(&edition.isbn10),
            // A provider reporting 0 pages is normalized to `None` upstream,
            // so anything here is a real count.
            MetadataField::PrintPages => edition.pages.map(|p| p.to_string()).unwrap_or_default(),
            MetadataField::Genres => list(&edition.genres),
            MetadataField::Description => text(&edition.description),
        }
    }

    /// Whether this source has anything to offer for this field.
    pub(super) fn is_available(self, edition: &ProviderEdition) -> bool {
        !self.source_value(edition).trim().is_empty()
    }

    /// Stage the source's value into the form's own signal.
    ///
    /// Staging only: the page's save bar and `on_save` stay the single writer,
    /// so dirty tracking, the field count, and validation keep working with no
    /// changes. A field the source lacks is a no-op — the guard is here as
    /// well as on the button, because "take everything" also comes through.
    pub(super) fn apply(self, fields: FormFields, edition: &ProviderEdition) {
        if !self.is_available(edition) {
            return;
        }
        let mut fields = fields;
        match self {
            MetadataField::Title => fields.title.set(self.source_value(edition)),
            // The list fields write the source's own vector rather than the
            // joined string the row displays.
            MetadataField::Authors => fields.authors.set(non_blank(&edition.authors)),
            MetadataField::Publisher => fields.publisher.set(self.source_value(edition)),
            MetadataField::Published => fields.published.set(self.source_value(edition)),
            MetadataField::Series => fields.series.set(self.source_value(edition)),
            MetadataField::Isbn13 => fields.isbn13.set(self.source_value(edition)),
            MetadataField::Isbn10 => fields.isbn10.set(self.source_value(edition)),
            MetadataField::PrintPages => fields.print_pages.set(self.source_value(edition)),
            MetadataField::Genres => fields.genres.set(non_blank(&edition.genres)),
            MetadataField::Description => fields.description.set(self.source_value(edition)),
        }
    }

    /// Stable slug for this field's `data-testid`s, matching the edit form's
    /// own label slugging without its `me-` prefix.
    pub(super) fn slug(self) -> String {
        super::field_slug(self.label())
    }
}

/// The list a list-valued field actually stages: trimmed, with blank entries
/// dropped, so what is written matches what the row displayed.
fn non_blank(values: &[String]) -> Vec<String> {
    values
        .iter()
        .map(|v| v.trim().to_string())
        .filter(|v| !v.is_empty())
        .collect()
}

#[cfg(test)]
mod tests;
