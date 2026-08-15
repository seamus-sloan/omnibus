//! Calibre `ebook-convert` integration: binary detection ([`detect`]) plus
//! the shell-out that converts a book ([`execute::convert_book`], driven by
//! `Task::ConvertFormat` in `crate::worker`). Calibre is strictly
//! optional — an unresolved binary logs one startup warning and leaves
//! conversion disabled.

mod detect;
mod execute;
mod fs;

pub use detect::{ebook_convert_available, ebook_convert_bin, is_runnable, warn_if_unavailable};
pub use execute::{convert_book, ConvertError};
pub use fs::{convert_dir, convert_path};

#[cfg(test)]
mod tests;
