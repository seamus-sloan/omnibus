//! Parsed ebook metadata + user-supplied override layer.
//!
//! `Contributor` / `Identifier` / `EbookMetadata` are the wire shape for a
//! single book row. `MetadataOverrides` is the JSON-serialized override
//! layer persisted in `metadata_overrides.overrides` and merged on read.
//! Re-exports flatten so callers keep `omnibus_shared::EbookMetadata` etc.

mod metadata;
mod overrides;

#[cfg(test)]
mod tests;

pub use metadata::{Contributor, EbookMetadata, Identifier};
pub use overrides::MetadataOverrides;
