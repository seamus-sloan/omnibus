//! Derive a human-readable locator from an EPUB CFI. A highlight stores only
//! its CFI range; the book-detail page maps that to the chapter the reader
//! names for the same position, and falls back to the raw spine "Section N"
//! otherwise.

use omnibus_shared::AlignmentEbookChapter;

use crate::pages::book_detail::chapter_ref;

/// Human locator for a highlight's CFI.
///
/// Names the **chapter's title**, not its ordinal. The two are not the same
/// number: a TOC counts front matter and part dividers, so the eighteenth
/// entry of Six of Crows is "Chapter 12: Inej" — and a kicker reading
/// "Chapter 18" beside a Resume line reading "Ch. 18 · Chapter 12" states a
/// chapter the book does not have (#2463). The ordinal is the reader's
/// position, which the resume readout already carries; a saved passage wants
/// the name.
///
/// Resolution is the CFI's spine step against the chapters' spine indices —
/// the same `chapter_ref` path the resume readout uses, so the two can't name
/// different chapters for one location (#2356). Falls back to the 1-based
/// spine "Section N" when no chapter structure is loaded, when the CFI sits
/// before the first chapter (front matter), or when the matched chapter
/// carries no title to print. `None` when the string has no readable spine
/// step — the caller then shows the saved date alone.
pub(super) fn highlight_locator(cfi: &str, chapters: &[AlignmentEbookChapter]) -> Option<String> {
    let titled = chapter_ref::chapter_index_for_cfi(&spine_indices(chapters), cfi)
        .and_then(|idx| chapters.get(idx))
        .map(|c| c.title.trim())
        .filter(|t| !t.is_empty());
    if let Some(title) = titled {
        return Some(title.to_string());
    }
    chapter_ref::cfi_spine_ordinal(cfi).map(|n| format!("Section {n}"))
}

/// Each chapter's 0-based `spine_index` in TOC order — the shape
/// [`chapter_ref::chapter_index_for_cfi`] resolves against.
fn spine_indices(chapters: &[AlignmentEbookChapter]) -> Vec<i64> {
    chapters.iter().map(|c| c.spine_index).collect()
}
