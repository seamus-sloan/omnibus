//! Book read path. Hydrates the normalized schema into the wire
//! `EbookMetadata` shape: scalar columns from `books`, single-valued joins
//! via scalar subqueries, multi-valued joins via `json_group_array`. Merges
//! `metadata_overrides` and backfills creator ids before returning.
//!
//! The implementation is split across focused sub-modules:
//!
//! * [`projection`] — shared column list, JSON row decoders, description
//!   sanitization, the `row -> EbookMetadata` mapper, and the
//!   `Contributor::id` backfill used after the override merge.
//! * [`list`] — library-scoped list/count read paths plus the small
//!   `IndexedRow` projection used by the incremental reindex diff.
//! * [`get`] — single-book read paths (`get_book`, `get_book_by_uuid`,
//!   `resolve_book_id_by_uuid`).
//! * [`search`] — FTS5-backed search and its companion count helpers.
//!
//! Public API is re-exported here so callers (`server/`, `frontend/`, sibling
//! `db/` modules) keep importing through `omnibus_db::books::*` unchanged.

mod facets;
mod get;
mod list;
mod page;
mod projection;
mod search;

#[cfg(test)]
mod tests;

pub use facets::library_facets;
pub use get::{
    book_file_path, book_file_path_by_id, get_book, get_book_by_uuid, get_book_files,
    resolve_book_id_by_uuid, resolve_book_id_by_uuid_exec, resolve_canonical_book_uuid,
    resolve_canonical_book_uuid_exec,
};
pub use list::{
    collect_paths, count_books, count_books_for_paths, library_from_db, library_from_db_combined,
    library_from_db_with_total, library_from_db_with_total_combined, list_books,
    list_books_for_paths, list_indexed_rows, list_indexed_rows_for_formats,
    list_merged_rows_for_formats, IndexedRow,
};
pub use page::{list_books_page, BookPage, CursorError, PageCursor};
pub use projection::MAX_BOOKS_RETURNED;
pub use search::{count_search_books, search_books, search_books_with_total};

/// Errors returned by the books read path.
#[derive(Debug, thiserror::Error)]
pub enum BooksError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
    /// Corrupt JSON in the `metadata_overrides` blob.
    #[error("overrides deserialization failed: {0}")]
    OverridesJson(serde_json::Error),
}

impl From<crate::metadata_overrides::MetadataOverridesError> for BooksError {
    fn from(e: crate::metadata_overrides::MetadataOverridesError) -> Self {
        match e {
            crate::metadata_overrides::MetadataOverridesError::Db(inner) => BooksError::Db(inner),
            crate::metadata_overrides::MetadataOverridesError::Serialization(inner) => {
                BooksError::OverridesJson(inner)
            }
        }
    }
}

// `pub(crate)` re-exports for sibling `db/` modules (`discovery`, `palette`,
// `metadata_overrides`) that referenced these items at `crate::books::…`
// before the split.
pub(crate) use get::{get_books_by_ids, resolve_book_ids_bulk};
pub(crate) use projection::{
    backfill_creator_ids, merge_overrides_into_books, parse_json_array, row_to_ebook,
    sanitize_description, BOOK_COLUMNS,
};
