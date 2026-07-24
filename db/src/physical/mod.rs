//! Physical ownership data layer: library-wide physical copies, the per-user
//! physical wishlist, and fileless book creation from external metadata.
//!
//! Copies and wishlist entries soft-reference `books.uuid` (no FK/cascade), so
//! a reindex that ghosts a book keeps ownership and wishlist intact. Checking
//! in a copy ([`add_physical_copy`]) fulfills every user's wishlist for that
//! book atomically. Physical-only books have no file: [`create_fileless_book`]
//! mints one under a synthetic "Physical" scan root.

mod copies;
mod fileless;
mod wishlist;

#[cfg(test)]
mod tests;

pub use copies::{add_physical_copy, delete_physical_copy, list_physical_copies};
pub use fileless::{create_fileless_book, FilelessBook, FilelessCover};
pub use wishlist::{add_wishlist_entry, list_wishlist, remove_wishlist_entry, LIST_WISHLIST_LIMIT};

/// Errors from the physical ownership data layer.
#[derive(Debug, thiserror::Error)]
pub enum PhysicalError {
    /// No live book resolves from the given uuid (honoring `merged_uuids`).
    #[error("book not found")]
    BookNotFound,
    /// The physical copy id does not exist.
    #[error("physical copy not found")]
    CopyNotFound,
    /// A cover file could not be written for a fileless book.
    #[error("cover write failed: {0}")]
    Cover(#[from] std::io::Error),
    #[error(transparent)]
    Books(#[from] crate::books::BooksError),
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}
