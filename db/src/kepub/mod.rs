//! EPUB→KEPUB conversion cache for the "Send to Kobo" download.
//!
//! Shells out to `kepubify` (via `$OMNIBUS_KEPUBIFY_PATH` or `$PATH`) to cache a
//! Kobo-optimized KEPUB at `<cache dir>/<book_id>.kepub.epub`, where the cache
//! dir is `$OMNIBUS_KEPUB_DIR` (default `$OMNIBUS_DATA_DIR/kepub`). Invalidated
//! on `books.last_modified` (like thumbs). Absent → plain EPUB.

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
