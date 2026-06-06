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

mod get;
mod list;
mod projection;
mod search;

#[cfg(test)]
mod tests;

pub use get::{book_file_path, get_book, get_book_by_uuid, resolve_book_id_by_uuid};
pub use list::{
    count_books, count_books_for_paths, library_from_db, library_from_db_combined,
    library_from_db_with_total, library_from_db_with_total_combined, list_books,
    list_books_for_paths, list_indexed_rows, list_indexed_rows_for_formats, IndexedRow,
};
pub use projection::MAX_BOOKS_RETURNED;
pub use search::{count_search_books, search_books, search_books_with_total};

/// Errors returned by the book read paths (`get_book`, `count_books`).
/// The single transparent `Db` variant honors the `02-error-handling`
/// boundary rule (no raw `sqlx::Error` across the module boundary) while
/// keeping `?` propagation clean at the call sites that still use sqlx
/// internally.
#[derive(Debug, thiserror::Error)]
pub enum BooksError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

// `pub(crate)` re-exports for sibling `db/` modules (`discovery`, `palette`,
// `metadata_overrides`) that referenced these items at `crate::books::…`
// before the split.
pub(crate) use projection::{
    backfill_creator_ids, parse_json_array, row_to_ebook, sanitize_description, BOOK_COLUMNS,
};
