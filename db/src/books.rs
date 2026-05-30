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
    count_books, library_from_db, library_from_db_with_total, list_books, list_indexed_rows,
    IndexedRow,
};
pub use projection::MAX_BOOKS_RETURNED;
pub use search::{count_search_books, search_books, search_books_with_total};

// `pub(crate)` re-exports for sibling `db/` modules (`discovery`, `palette`,
// `metadata_overrides`) that referenced these items at `crate::books::…`
// before the split.
pub(crate) use projection::{
    backfill_creator_ids, parse_json_array, row_to_ebook, sanitize_description, BOOK_COLUMNS,
};
