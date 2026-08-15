//! Calibre `ebook-convert` integration: binary detection ([`detect`]) plus
//! the shell-out that converts a book ([`execute::convert_book`], driven by
//! `Task::ConvertFormat` in `crate::worker`), which persists its output as a
//! new `book_files` row ([`persist::persist_converted_file`], #949) so the
//! converted format shows up alongside the original everywhere a book's
//! files are listed. Calibre is strictly optional — an unresolved binary
//! logs one startup warning and leaves conversion disabled.

mod detect;
mod execute;
mod fs;
mod persist;

pub use detect::{ebook_convert_available, ebook_convert_bin, is_runnable, warn_if_unavailable};
pub use execute::{convert_book, ConvertError};
pub use fs::{convert_dir, convert_path};

#[cfg(test)]
mod tests;
