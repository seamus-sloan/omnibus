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

pub use cfi::{format_cfi, format_range_cfi, parse_cfi, parse_range_cfi, Cfi, CfiRange, CfiTail};
pub use location::{
    annotation_location_json, location_json, parse_annotation_location, parse_location,
    parse_span_id, span_id, KoboLoc, KoboSpanRange, SpanPoint,
};

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

/// Kobo→Web: derive epub.js-compatible *range* CFIs for a batch of
/// annotation anchors from one book, opening each container (and indexing
/// each content document) once. Per-anchor failures — unknown source,
/// missing span, snippet mismatch, an inverted range — yield `None` in that
/// slot rather than failing the batch; `Err` is reserved for not being able
/// to open either container at all.
pub fn annotation_cfis(
    kepub: &Path,
    source_epub: &Path,
    ranges: &[KoboSpanRange],
) -> anyhow::Result<Vec<Option<String>>> {
    if ranges.is_empty() {
        return Ok(Vec::new());
    }
    let mut kdoc = book::open_doc(kepub)?;
    let mut sdoc = book::open_doc(source_epub)?;
    let mut k_cache: std::collections::HashMap<usize, walk::FileIndex> = Default::default();
    let mut s_cache: std::collections::HashMap<usize, walk::FileIndex> = Default::default();
    Ok(ranges
        .iter()
        .map(|r| annotation_cfi(&mut kdoc, &mut sdoc, &mut k_cache, &mut s_cache, r))
        .collect())
}

/// One anchor of [`annotation_cfis`]: kepub walk for both endpoints, source
/// walk with the mandatory snippet-equality proof at each, then the range
/// CFI. Any miss is a `None`.
fn annotation_cfi(
    kdoc: &mut book::Doc,
    sdoc: &mut book::Doc,
    k_cache: &mut std::collections::HashMap<usize, walk::FileIndex>,
    s_cache: &mut std::collections::HashMap<usize, walk::FileIndex>,
    range: &KoboSpanRange,
) -> Option<String> {
    let k_idx = book::spine_index_for_source(kdoc, &range.source)?;
    let k_index = cached_index(kdoc, k_cache, k_idx)?;
    // Start: land forward on the first highlighted character. End: the
    // exclusive boundary after the last one (`span_boundary` + `tail_end_at`
    // resolve backward), so a highlight ending at a span's edge never bleeds
    // into the following paragraph.
    let start_boundary =
        k_index.span_boundary(range.start.n, range.start.m, range.start.char_utf16)?;
    let end_boundary = k_index.span_boundary(range.end.n, range.end.m, range.end.char_utf16)?;
    let k_start = k_index.anchor_at(start_boundary)?;
    let k_end_check = k_index.back_anchor_before(end_boundary)?;
    if k_end_check.norm_offset < k_start.norm_offset {
        return None;
    }

    let s_idx = book::spine_index_for_source(sdoc, &range.source)?;
    let s_index = cached_index(sdoc, s_cache, s_idx)?;
    let (start_tail, s_start) = s_index.tail_at(k_start.norm_offset)?;
    if s_start.norm_offset != k_start.norm_offset || s_start.snippet != k_start.snippet {
        return None;
    }
    let (end_tail, s_end) = s_index.tail_end_at(end_boundary)?;
    if s_end.norm_offset != k_end_check.norm_offset || s_end.snippet != k_end_check.snippet {
        return None;
    }
    Some(format_range_cfi(s_idx, &start_tail, &end_tail))
}

/// Web→Kobo: derive Kobo annotation `location` objects for a batch of
/// stored range CFIs from one book, opening each container (and indexing
/// each content document) once — the inverse of [`annotation_cfis`].
/// Per-anchor failures — an unparseable or point CFI, unknown spine index,
/// missing kepub span, snippet mismatch — yield `None` in that slot rather
/// than failing the batch; `Err` is reserved for not being able to open
/// either container at all.
pub fn annotation_locations(
    kepub: &Path,
    source_epub: &Path,
    cfis: &[String],
) -> anyhow::Result<Vec<Option<String>>> {
    if cfis.is_empty() {
        return Ok(Vec::new());
    }
    let mut kdoc = book::open_doc(kepub)?;
    let mut sdoc = book::open_doc(source_epub)?;
    let mut k_cache: std::collections::HashMap<usize, walk::FileIndex> = Default::default();
    let mut s_cache: std::collections::HashMap<usize, walk::FileIndex> = Default::default();
    Ok(cfis
        .iter()
        .map(|cfi| annotation_location(&mut kdoc, &mut sdoc, &mut k_cache, &mut s_cache, cfi))
        .collect())
}

/// One anchor of [`annotation_locations`]: source walk for both endpoints,
/// kepub walk with the mandatory snippet-equality proof at each, then the
/// span-range location. Any miss is a `None`.
fn annotation_location(
    kdoc: &mut book::Doc,
    sdoc: &mut book::Doc,
    k_cache: &mut std::collections::HashMap<usize, walk::FileIndex>,
    s_cache: &mut std::collections::HashMap<usize, walk::FileIndex>,
    cfi: &str,
) -> Option<String> {
    let range = parse_range_cfi(cfi)?;
    if range.spine_index >= sdoc.spine.len() {
        return None;
    }
    let href = book::spine_href(sdoc, range.spine_index)?;
    let s_index = cached_index(sdoc, s_cache, range.spine_index)?;
    let s_start = s_index.offset_at(&range.start)?;
    let s_end = s_index.offset_end_at(&range.end)?;
    if s_end.norm_offset < s_start.norm_offset {
        return None;
    }

    let k_idx = book::spine_index_for_source(kdoc, &href)?;
    let k_index = cached_index(kdoc, k_cache, k_idx)?;
    // The proof of alignment, at both endpoints: the kepub walk must see
    // the same following text at the same normalized offsets, or the
    // derivation is abandoned.
    if k_index.snippet_at(s_start.norm_offset) != s_start.snippet
        || k_index.snippet_at(s_end.norm_offset) != s_end.snippet
    {
        return None;
    }
    let start = k_index.span_point_at(s_start.norm_offset)?;
    let end = k_index.span_point_after(s_end.norm_offset)?;
    // The device resolves `chapterFilename` against its downloaded KEPUB,
    // so the kepub's own spine href is the one to carry.
    let k_href = book::spine_href(kdoc, k_idx)?;
    Some(annotation_location_json(&k_href, start, end))
}

/// Visible-character count of one spine document — the same walk and
/// normalization every position derivation uses, exposed for the spine-stats
/// extraction so stored stats and live offsets share one coordinate system.
pub(crate) fn visible_chars(xhtml: &[u8]) -> anyhow::Result<u64> {
    Ok(walk::index_file(xhtml)?.visible_char_count())
}

/// Spine index + normalized visible-text offset of a CFI within its own
/// spine document — the single-file half of a percent derivation, for
/// callers holding whole-book spine stats (`epub_spine_stats`), who then
/// finish with O(1) arithmetic instead of the full-book walk. `Ok(None)`
/// for every underivable case.
pub(crate) fn cfi_spine_offset(
    source_epub: &Path,
    cfi: &str,
) -> anyhow::Result<Option<(usize, u64)>> {
    let Some(parsed) = parse_cfi(cfi) else {
        return Ok(None);
    };
    let mut sdoc = book::open_doc(source_epub)?;
    if parsed.spine_index >= sdoc.spine.len() {
        return Ok(None);
    }
    let bytes = book::read_spine_entry(&mut sdoc, parsed.spine_index)?;
    let index = walk::index_file(&bytes)?;
    let Some(anchor) = index.offset_at(&parsed.tail) else {
        return Ok(None);
    };
    Ok(Some((parsed.spine_index, anchor.norm_offset)))
}

/// Read + index spine entry `idx`, memoized per batch so a book with many
/// annotations in one chapter parses that chapter once.
fn cached_index<'c>(
    doc: &mut book::Doc,
    cache: &'c mut std::collections::HashMap<usize, walk::FileIndex>,
    idx: usize,
) -> Option<&'c walk::FileIndex> {
    if let std::collections::hash_map::Entry::Vacant(slot) = cache.entry(idx) {
        let bytes = book::read_spine_entry(doc, idx).ok()?;
        slot.insert(walk::index_file(&bytes).ok()?);
    }
    cache.get(&idx)
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
