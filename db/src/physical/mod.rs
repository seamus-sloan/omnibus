//! Physical ownership data layer: library-wide physical copies, the per-user
//! physical wishlist, and fileless book creation from external metadata.
//! Copies and wishlist entries soft-reference `books.uuid` (no FK/cascade), so
//! a reindex that ghosts a book keeps them intact, and checking in a copy
//! ([`add_physical_copy`]) fulfills every user's wishlist for that book atomically.

mod copies;
mod fileless;
mod promote;
mod remove;
mod wishlist;

#[cfg(test)]
mod tests;

pub use copies::{
    add_physical_copy, delete_physical_copy, list_physical_copies,
    list_physical_copies_by_canonical_uuid_exec, update_physical_copy_note,
};
pub use fileless::{create_fileless_book, FilelessBook, FilelessCover};
pub(crate) use promote::{promote_filed_physical_book, promote_filed_physical_books};
pub use remove::delete_fileless_book;
pub use wishlist::{
    add_wishlist_entry, get_wishlist_entry, list_wishlist, remove_wishlist_entry,
    LIST_WISHLIST_LIMIT,
};

/// Sentinel scan-root path that owns every fileless (physical-only) book. It is
/// never a scan target, so ebook/audiobook reindex diffs never touch these
/// rows; the never-prune guard keeps the row alive while any physical book
/// references it. [`promote_filed_physical_book`] moves a book off it the
/// moment a real library file attaches — a book left here is invisible to
/// every path-scoped read (All Books, search) unless a physical copy backs it.
pub(crate) const PHYSICAL_LIBRARY_PATH: &str = "physical://local";

/// Errors from the physical ownership data layer.
#[derive(Debug, thiserror::Error)]
pub enum PhysicalError {
    /// No live book resolves from the given uuid (honoring `merged_uuids`).
    #[error("book not found")]
    BookNotFound,
    /// The physical copy id does not exist.
    #[error("physical copy not found")]
    CopyNotFound,
    /// Refused: the book still has digital files, so the reindex diff owns it.
    #[error("book still has files")]
    BookHasFiles,
    /// A cover file could not be written for a fileless book.
    #[error("cover write failed: {0}")]
    Cover(#[from] std::io::Error),
    /// A book-layer failure surfaced from `crate::books`.
    #[error(transparent)]
    Books(#[from] crate::books::BooksError),
    /// A database error not covered by a more specific variant above.
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
    /// A non-database failure surfaced from a dependency of these writes.
    /// Coarse and message-carrying: callers render it as an opaque internal
    /// failure, so folding it into `Sqlx` would only make that variant mean
    /// "a database error occurred" less than always.
    #[error("{0}")]
    Other(String),
}
