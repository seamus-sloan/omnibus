//! Derive a human-readable locator from an EPUB CFI. A highlight stores only
//! its CFI range; the book-detail page maps that to the reader's chapter
//! number when the book's chapter structure is loaded, and falls back to the
//! raw spine "Section N" otherwise.

use crate::pages::book_detail::chapter_ref;

/// Human locator for a highlight's CFI. Prefers the reader's chapter number —
/// the CFI's spine step mapped to the book's TOC chapters, where
/// `chapter_spines` is each chapter's 0-based `spine_index` in TOC order — so a
/// saved passage and the reader name the same chapter rather than a raw spine
/// ordinal that counts front matter (#2356). Falls back to the 1-based spine
/// "Section N" when no chapter structure is loaded, or when the CFI sits before
/// the first chapter (front matter). `None` when the string carries no
/// readable spine step — the caller then shows the saved date alone.
pub(super) fn highlight_locator(cfi: &str, chapter_spines: &[i64]) -> Option<String> {
    if let Some(idx) = chapter_ref::chapter_index_for_cfi(chapter_spines, cfi) {
        return Some(format!("Chapter {}", idx + 1));
    }
    chapter_ref::cfi_spine_ordinal(cfi).map(|n| format!("Section {n}"))
}
