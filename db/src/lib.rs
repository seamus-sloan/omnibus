//! Omnibus data layer: schema migrations, SQLite pool init, the normalized
//! query layer, and the indexing pipeline (file scan → EPUB metadata
//! extraction → atomic per-library upsert). Consumed by both `server/`
//! (axum REST handlers) and `frontend/` (Dioxus server functions under
//! `feature = "server"`).

pub mod audiobook;
pub mod auth;
pub mod author_photos;
pub mod author_photos_data;
pub mod books;
pub mod browse;
pub mod covers;
pub mod discovery;
pub mod ebook;
pub mod helpers;
pub mod highlights;
pub mod hls;
pub mod identity;
pub mod indexer;
pub mod library_layout;
pub mod merge;
pub mod metadata_overrides;
pub mod missing_files;
pub mod normalize;
pub mod palette;
pub mod pool;
pub mod progress;
pub mod scanner;
pub mod settings;
pub mod sort_keys;
pub mod sync;
mod taxonomy;
#[cfg(any(test, feature = "test-support"))]
pub mod test_support;
pub mod thumbs;
pub mod worker;

// Flatten the query layer so callers write `omnibus_db::list_books(...)`
// instead of `omnibus_db::queries::list_books(...)`. Keeps callsites terse
// and mirrors how `db.rs` looked before the extraction.
pub use author_photos_data::*;
pub use books::{
    book_file_path, book_file_path_by_id, collect_paths, count_books, count_books_for_paths,
    count_search_books, get_book, get_book_by_uuid, get_book_files, library_facets,
    library_from_db, library_from_db_combined, library_from_db_with_total,
    library_from_db_with_total_combined, list_books, list_books_for_paths, list_books_page,
    list_indexed_rows, list_indexed_rows_for_formats, list_merged_rows_for_formats,
    resolve_book_id_by_uuid, resolve_book_id_by_uuid_exec, resolve_canonical_book_uuid,
    resolve_canonical_book_uuid_exec, search_books, search_books_with_total, BookPage, BooksError,
    CursorError, IndexedRow, PageCursor, MAX_BOOKS_RETURNED,
};
pub use browse::*;
pub use covers::{covers_dir, get_cover, get_last_modified_epoch, CoversError};
pub use discovery::*;
pub use helpers::{build_fts_match, sanitize_fts_query};
pub use merge::{merge_books, undo_merge, MergeError, MergeOutcome};
pub use metadata_overrides::*;
pub use missing_files::{
    backfill_missing_files_flags, gc_books_missing_files, MissingFilesError,
    MISSING_FILES_RETENTION_DAYS,
};
pub use palette::*;
pub use pool::*;
pub use settings::*;
pub use sort_keys::{backfill_series_sort, series_sort_value, SortKeysError};
pub use sync::*;
pub use thumbs::{thumb_path_for, thumbs_dir, ThumbError, ThumbSize};
