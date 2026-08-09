//! Calibre `ebook-convert` integration. Today this is detection only: where
//! the binary lives (`$OMNIBUS_EBOOK_CONVERT_PATH`, else `$PATH`) and whether
//! it is runnable. Calibre is strictly optional — an unresolved binary logs
//! one startup warning and leaves conversion disabled. The admin-overridable
//! `ebook_convert_path` setting lives in `crate::settings::ebook_convert`.

mod detect;

pub use detect::{ebook_convert_available, ebook_convert_bin, is_runnable, warn_if_unavailable};

#[cfg(test)]
mod tests;
