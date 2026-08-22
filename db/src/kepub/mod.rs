//! EPUB→KEPUB conversion cache for the "Send to Kobo" download. Shells out to
//! `kepubify` (via `$OMNIBUS_KEPUBIFY_PATH` or `$PATH`) to cache a
//! Kobo-optimized KEPUB at `<cache dir>/<book_id>.kepub.epub`, invalidated on
//! `books.last_modified` like thumbs. An absent kepubify falls back to plain EPUB.

mod convert;
mod detect;
mod fs;

pub use convert::convert_book;
pub use detect::{kepubify_available, warn_if_unavailable};
pub use fs::{is_stale, kepub_dir, kepub_path};

/// Errors from the EPUB→KEPUB conversion path.
///
/// `BookNotFound`/`SourceMissing` stay distinct variants because
/// `worker::handlers::kepub_outcome` branches on them to decide whether the
/// message is safe to hand back verbatim. Everything else — DB lookup
/// failures, process spawn / filesystem errors, and a non-zero kepubify exit
/// — is foreign-system failure with no caller branching on the specific
/// cause, so it collapses into `Failed` (rule 02: anyhow-territory).
#[derive(Debug, thiserror::Error)]
pub enum KepubError {
    #[error("book {0} not found")]
    BookNotFound(i64),
    #[error("book {0} has no EPUB file to convert")]
    SourceMissing(i64),
    #[error("kepub conversion failed: {0}")]
    Failed(#[from] anyhow::Error),
}

#[cfg(test)]
mod tests;
