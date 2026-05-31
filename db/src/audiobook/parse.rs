//! Audiobook tag extraction via `lofty`.
//!
//! Opens each file's primary tag and lifts the small set of fields the
//! basic player cares about: title, primary artist (one author — the
//! basic player has no UI for narrator vs. author roles), album, and
//! duration in seconds. Failures roll up as [`AudiobookError`] so the
//! indexer surfaces the per-file error in the same shape the EPUB path
//! uses.

use std::path::{Path, PathBuf};

use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;

/// Predictable failure space for the audiobook parse + indexer dispatch.
/// `Io` covers "could not open file"; `Tag` covers any lofty decode /
/// container error; `Unsupported` is reserved for files whose extension
/// we accepted but lofty can't probe.
#[derive(Debug, thiserror::Error)]
pub enum AudiobookError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("tag decode failed: {0}")]
    Tag(#[from] lofty::error::LoftyError),
    #[error("unsupported audiobook format: {0}")]
    Unsupported(String),
}

/// Tag-only metadata view used by [`super::build_indexed_book`]. Empty
/// strings collapse to `None` so downstream defaults (filename-stem fall-
/// back for title, no-author for missing artist) kick in uniformly.
#[derive(Debug, Default, Clone)]
pub struct AudiobookMetadata {
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub duration_seconds: Option<f64>,
}

/// Open `path` with lofty and lift the basic-player tag fields. Caller
/// supplies the extension check upstream (via [`super::AUDIOBOOK_EXTENSIONS`]
/// in the scanner), so this never sees a non-audio file under normal
/// operation.
pub(super) fn extract_metadata(path: &Path) -> Result<AudiobookMetadata, AudiobookError> {
    let tagged = lofty::read_from_path(path)?;
    let duration_seconds =
        Some(tagged.properties().duration().as_secs_f64()).filter(|d| d.is_finite() && *d > 0.0);
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
    let mut out = AudiobookMetadata {
        duration_seconds,
        ..Default::default()
    };
    if let Some(tag) = tag {
        out.title = tag
            .title()
            .map(|c| c.trim().to_string())
            .filter(|s| !s.is_empty());
        out.artist = tag
            .artist()
            .map(|c| c.trim().to_string())
            .filter(|s| !s.is_empty());
        out.album = tag
            .album()
            .map(|c| c.trim().to_string())
            .filter(|s| !s.is_empty());
    }
    Ok(out)
}

/// Phase B input: one entry per audiobook file the diff says needs a
/// full tag parse (the New + Changed buckets). Mirrors
/// [`crate::ebook::ParseTarget`].
#[derive(Debug, Clone)]
pub struct AudiobookParseTarget {
    pub filename: String,
    pub absolute: PathBuf,
    pub mtime_epoch: i64,
    pub size_bytes: i64,
}

/// Phase B: parse each target sequentially and emit one
/// [`super::IndexedBook`] per success. A per-file parse failure surfaces
/// as an `IndexedBook` whose metadata carries `error = Some(_)` — same
/// shape the EPUB path uses so one bad file does not hide the rest of
/// the library.
pub fn parse_audiobook_targets(targets: Vec<AudiobookParseTarget>) -> Vec<super::IndexedBook> {
    targets
        .into_iter()
        .map(|t| {
            let mut book = match super::build_indexed_book(&t.absolute, t.filename.clone()) {
                Ok(b) => b,
                Err(e) => super::IndexedBook {
                    metadata: omnibus_shared::EbookMetadata {
                        filename: t.filename,
                        error: Some(format!("could not read audiobook: {e}")),
                        ..Default::default()
                    },
                    cover: None,
                    mtime_epoch: 0,
                    size_bytes: 0,
                },
            };
            book.mtime_epoch = t.mtime_epoch;
            book.size_bytes = t.size_bytes;
            book
        })
        .collect()
}
