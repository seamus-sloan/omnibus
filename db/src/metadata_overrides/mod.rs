//! Metadata overrides: the user-editable layer on top of the canonical
//! scanned metadata. Persists `MetadataOverrides` JSON per `book_uuid`
//! and merges it on top of the read path. Split into focused submodules
//! for the SQL CRUD layer, the post-write FTS rebuild, and the
//! series/author/tag link materialization helpers.

// Submodules are private — `db/src/lib.rs` does
// `pub use metadata_overrides::*`, so any `pub mod` here would expose
// `omnibus_db::upsert`, etc. to downstream crates, which is a new
// public path that didn't exist before the split. Matches the
// leaf-submodule-private precedent in `db/src/discovery/mod.rs`; only
// the named items are re-exported below.
mod fts;
mod links;
mod upsert;

#[cfg(test)]
mod tests;

pub(crate) use upsert::{apply_overrides, load_overrides_bulk};
pub use upsert::{
    delete_metadata_overrides, delete_override_cover, get_book_uuid, get_metadata_overrides,
    merge_metadata_overrides, upsert_metadata_overrides, write_override_cover,
    MetadataOverridesError,
};

// `pub(crate)` re-export so sibling `db/` modules and the colocated tests
// keep importing `rebuild_fts_for_book` through `crate::metadata_overrides`
// after the split. Not part of the downstream public surface.
pub(crate) use fts::rebuild_fts_for_book;
pub(crate) use fts::rebuild_fts_for_books_batch;
