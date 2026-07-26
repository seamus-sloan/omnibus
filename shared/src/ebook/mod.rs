//! Parsed ebook metadata + user-supplied override layer.
//!
//! `Contributor` / `Identifier` / `EbookMetadata` are the wire shape for a
//! single book row. `MetadataOverrides` is the JSON-serialized override
//! layer persisted in `metadata_overrides.overrides` and merged on read.
//! Re-exports flatten so callers keep `omnibus_shared::EbookMetadata` etc.

mod export;
mod metadata;
mod overrides;

#[cfg(test)]
mod tests;

pub use export::OpfExportResult;
pub use metadata::{BookFileInfo, Contributor, EbookMetadata, Identifier};
pub use overrides::MetadataOverrides;

/// Resolves a display title: the given title if set, otherwise the filename.
pub fn display_title(title: Option<&str>, filename: &str) -> String {
    title
        .map(str::to_owned)
        .unwrap_or_else(|| filename.to_owned())
}
