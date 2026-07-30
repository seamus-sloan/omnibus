//! Bidirectional translation between the two reading-position languages:
//! Kobo `KoboSpan` bookmarks (`kobo.N.M` ids kepubify injects into the
//! cached KEPUB) and the EPUB CFIs the web/iOS readers speak. The cached
//! KEPUB is the Rosetta stone: both variants carry the same visible text,
//! so a position in one maps to the other by aligned text offset. Callers
//! are the Kobo sync handlers in `server::backend::kobo`; every failure
//! degrades to "no derived position", never a wrong one — a mandatory
//! snippet-equality check between the two walks enforces that.

mod book;
mod cfi;
mod location;
#[cfg(test)]
mod tests;
mod walk;

use std::path::Path;

pub use cfi::{format_cfi, parse_cfi, Cfi, CfiTail};
pub use location::{location_json, parse_location, parse_span_id, span_id, KoboLoc};

/// Web→Kobo derivation result. The span needs the kepub cache; the percent
/// needs only the source EPUB, so a cache miss degrades to a truthful
/// percent-only bookmark rather than nothing.
#[derive(Debug, Default)]
pub struct DerivedSpan {
    /// `CurrentBookmark.Location` JSON, when the kepub walk succeeded.
    pub location_json: Option<String>,
    /// Whole-book visible-text percent, 0..=100.
    pub percent: Option<i64>,
}

/// Kobo→Web: derive an epub.js-compatible CFI for a KoboSpan bookmark.
///
/// `Ok(None)` covers every underivable case — unknown `Source`, missing
/// span id, or a snippet mismatch proving the kepub and source text
/// streams diverged (kepubify normalization, or a replaced source file
/// under a stale cache). `Err` is reserved for I/O-level surprises.
pub fn span_to_cfi(
    kepub: &Path,
    source_epub: &Path,
    loc: &KoboLoc,
) -> anyhow::Result<Option<String>> {
    let mut kdoc = book::open_doc(kepub)?;
    let Some(k_idx) = book::spine_index_for_source(&kdoc, &loc.source) else {
        return Ok(None);
    };
    let k_bytes = book::read_spine_entry(&mut kdoc, k_idx)?;
    let k_index = walk::index_file(&k_bytes)?;
    let Some(anchor) = k_index.span_start(loc.n, loc.m) else {
        return Ok(None);
    };

    let mut sdoc = book::open_doc(source_epub)?;
    let Some(s_idx) = book::spine_index_for_source(&sdoc, &loc.source) else {
        return Ok(None);
    };
    let s_bytes = book::read_spine_entry(&mut sdoc, s_idx)?;
    let s_index = walk::index_file(&s_bytes)?;
    let Some((tail, s_anchor)) = s_index.tail_at(anchor.norm_offset) else {
        return Ok(None);
    };
    // The proof of alignment: both walks must see the same following text
    // at the same normalized offset, or the derivation is abandoned.
    if s_anchor.norm_offset != anchor.norm_offset || s_anchor.snippet != anchor.snippet {
        return Ok(None);
    }
    Ok(Some(format_cfi(s_idx, &tail)))
}

/// Web→Kobo: derive a KoboSpan location and whole-book percent for a
/// stored CFI. `kepub: None` (cache absent) still yields the percent half.
pub fn cfi_to_span(
    kepub: Option<&Path>,
    source_epub: &Path,
    cfi: &str,
) -> anyhow::Result<DerivedSpan> {
    let Some(parsed) = parse_cfi(cfi) else {
        return Ok(DerivedSpan::default());
    };
    let mut sdoc = book::open_doc(source_epub)?;
    if parsed.spine_index >= sdoc.spine.len() {
        return Ok(DerivedSpan::default());
    }
    let Some(href) = book::spine_href(&sdoc, parsed.spine_index) else {
        return Ok(DerivedSpan::default());
    };
    let s_bytes = book::read_spine_entry(&mut sdoc, parsed.spine_index)?;
    let s_index = walk::index_file(&s_bytes)?;
    let Some(anchor) = s_index.offset_at(&parsed.tail) else {
        return Ok(DerivedSpan::default());
    };
    let percent = book_percent(&mut sdoc, parsed.spine_index, &s_index, anchor.norm_offset)?;

    let location_json = match kepub {
        Some(kepub) => derive_span_half(kepub, &href, &anchor).unwrap_or_else(|e| {
            tracing::warn!(target: "omnibus::kobo_position", error = %e, "kepub span walk failed");
            None
        }),
        None => None,
    };
    Ok(DerivedSpan {
        location_json,
        percent: Some(percent),
    })
}

/// The kepub half of [`cfi_to_span`]: find the span covering the source
/// anchor and prove alignment by snippet at the same offset.
fn derive_span_half(
    kepub: &Path,
    href: &str,
    anchor: &walk::Anchor,
) -> anyhow::Result<Option<String>> {
    let mut kdoc = book::open_doc(kepub)?;
    let Some(k_idx) = book::spine_index_for_source(&kdoc, href) else {
        return Ok(None);
    };
    let k_bytes = book::read_spine_entry(&mut kdoc, k_idx)?;
    let k_index = walk::index_file(&k_bytes)?;
    if k_index.snippet_at(anchor.norm_offset) != anchor.snippet {
        return Ok(None);
    }
    let Some((n, m)) = k_index.span_at(anchor.norm_offset) else {
        return Ok(None);
    };
    Ok(Some(location_json(href, n, m)))
}

/// Whole-book percent of the anchor: visible chars before the spine item
/// plus the intra-file offset, over the total. Floor semantics, clamped.
fn book_percent(
    sdoc: &mut book::Doc,
    spine_index: usize,
    current: &walk::FileIndex,
    offset_in_file: u64,
) -> anyhow::Result<i64> {
    let mut before: u64 = 0;
    let mut total: u64 = 0;
    for i in 0..sdoc.spine.len() {
        let count = if i == spine_index {
            current.visible_char_count()
        } else {
            let bytes = book::read_spine_entry(sdoc, i)?;
            walk::index_file(&bytes)
                .map(|idx| idx.visible_char_count())
                .unwrap_or(0)
        };
        if i < spine_index {
            before += count;
        }
        total += count;
    }
    if total == 0 {
        return Ok(0);
    }
    Ok(((100 * (before + offset_in_file)) / total).clamp(0, 100) as i64)
}
