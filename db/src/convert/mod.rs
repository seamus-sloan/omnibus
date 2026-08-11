//! Calibre `ebook-convert` integration: binary detection (where it lives —
//! `$OMNIBUS_EBOOK_CONVERT_PATH`, else `$PATH` — and whether it is runnable),
//! plus the shell-out that actually converts a book
//! ([`execute::convert_book`], driven by `Task::ConvertFormat` in
//! `crate::worker`). Calibre is strictly optional — an unresolved binary
//! logs one startup warning and leaves conversion disabled. The
//! admin-overridable `ebook_convert_path` setting lives in
//! `crate::settings::ebook_convert`.

mod detect;
mod execute;
mod fs;

pub use detect::{ebook_convert_available, ebook_convert_bin, is_runnable, warn_if_unavailable};
pub use execute::{convert_book, ConvertError};
pub use fs::{convert_dir, convert_path};

#[cfg(test)]
mod tests;
