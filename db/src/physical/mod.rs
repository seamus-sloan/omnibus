//! Physical ownership data layer: library-wide physical copies, the per-user
//! physical wishlist, and fileless book creation from external metadata.
//! Copies and wishlist entries soft-reference `books.uuid` (no FK/cascade), so
//! a reindex that ghosts a book keeps them intact, and checking in a copy
//! ([`add_physical_copy`]) fulfills every user's wishlist for that book atomically.

mod copies;
mod fileless;
mod remove;
mod wishlist;

#[cfg(test)]
mod tests;

pub use copies::{
    add_physical_copy, delete_physical_copy, list_physical_copies, update_physical_copy_note,
};
pub use fileless::{create_fileless_book, FilelessBook, FilelessCover};
pub use remove::delete_fileless_book;
pub use wishlist::{
    add_wishlist_entry, get_wishlist_entry, list_wishlist, remove_wishlist_entry,
    LIST_WISHLIST_LIMIT,
};

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
}
