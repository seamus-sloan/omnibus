//! Parsed ebook metadata + user-supplied override layer.
//!
//! `Contributor` / `Identifier` / `EbookMetadata` are the wire shape for a
//! single book row. `MetadataOverrides` is the JSON-serialized override
//! layer persisted in `metadata_overrides.overrides` and merged on read.
//! Re-exports flatten so callers keep `omnibus_shared::EbookMetadata` etc.

mod metadata;
mod overrides;
mod validator;

#[cfg(test)]
mod tests;

pub use metadata::{BookFileInfo, Contributor, EbookMetadata, Identifier};
pub use overrides::MetadataOverrides;
pub use validator::{
    file_etag, DownloadFormat, DownloadValidator, DownloadValidatorQuery, DownloadValidatorRequest,
    DownloadValidatorResponse, MAX_VALIDATOR_QUERY,
};

/// Resolves a display title: the given title if set, otherwise the filename.
pub fn display_title(title: Option<&str>, filename: &str) -> String {
    title
        .map(str::to_owned)
        .unwrap_or_else(|| filename.to_owned())
}
