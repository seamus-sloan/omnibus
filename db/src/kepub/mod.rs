//! EPUB→KEPUB conversion cache for Kobo sideload downloads.
//!
//! Shells out to `kepubify` (from `$PATH`, or `$OMNIBUS_KEPUBIFY_PATH`) to
//! turn a book's EPUB into a Kobo-optimized KEPUB, caching the result at
//! `$OMNIBUS_DATA_DIR/kepub/<book_id>.kepub.epub`. Cache freshness follows
//! `books.last_modified`, mirroring the thumbnail cache. When kepubify is
//! absent the caller falls back to serving the plain EPUB.

mod convert;
mod detect;
mod fs;

pub use convert::convert_book;
pub use detect::{kepubify_available, warn_if_unavailable};
pub use fs::{is_stale, kepub_dir, kepub_path};

/// Errors from the EPUB→KEPUB conversion path.
///
/// The DB lookups (`get_last_modified_epoch`, `book_file_path`) surface as
/// their owning module's typed error; process spawn / filesystem failures
/// surface as `Io`; a kepubify run that exits non-zero surfaces as `NonZero`
/// so the caller can log stderr and fall back to plain EPUB.
#[derive(Debug, thiserror::Error)]
pub enum KepubError {
    #[error("book {0} not found")]
    BookNotFound(i64),
    #[error("book {0} has no EPUB file to convert")]
    SourceMissing(i64),
    #[error("kepubify exited with {status}: {stderr}")]
    NonZero { status: String, stderr: String },
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Books(#[from] crate::books::BooksError),
    #[error(transparent)]
    Covers(#[from] crate::CoversError),
}

#[cfg(test)]
mod tests;
