//! Resolve a reader chapter from an EPUB CFI's spine step. The book-detail
//! resume readout and the saved-passage locator both name the chapter the
//! reader opens, so they derive it the same way — from the CFI's spine item
//! against the chapters' spine indices — rather than the rounded whole-book
//! percent that lands one chapter ahead when a boundary falls inside the
//! rounding window (#2345, #2356).

/// 1-based spine index encoded in a CFI's pre-`!` package step.
///
/// A CFI opens with the path through the package document — `/6/14[chap3]!…`
/// — where `/6` selects the `<spine>` and the step after it is the *child*
/// index of the spine item. Child indices are even (odd steps address text
/// nodes), so the item's ordinal is that step halved.
pub(super) fn cfi_spine_ordinal(cfi: &str) -> Option<u32> {
    let inner = cfi.trim().strip_prefix("epubcfi(")?.strip_suffix(')')?;
    // Everything up to the first `!` is the package-document path; what follows
    // steps inside the spine item itself.
    let package = inner.split('!').next()?;
    let step = package.rsplit('/').find(|s| !s.is_empty())?;
    // Drop a trailing `[id]` assertion (`14[chap3]` → `14`).
    let digits = step.split('[').next()?;
    let raw: u32 = digits.parse().ok()?;
    // Odd or zero means this isn't an element step, so the halving would be a
    // fabrication. Report nothing instead.
    if raw == 0 || !raw.is_multiple_of(2) {
        return None;
    }
    Some(raw / 2)
}

/// Index of the chapter a CFI sits in: the last chapter whose 0-based spine
/// item is at or before the CFI's spine item. `spine_indices` is each
/// chapter's `spine_index` in TOC order (ascending). `None` when the CFI
/// carries no readable spine step, letting the caller fall back to the
/// percent-based estimate.
///
/// Matches the reader, which navigates by spine document: one chapter per
/// spine item resolves exactly, and the percent-rounding off-by-one across a
/// chapter boundary can no longer occur. Multiple chapters sharing a spine
/// item resolve to the last of them — still within the reader's open document.
pub(super) fn chapter_index_for_cfi(spine_indices: &[i64], cfi: &str) -> Option<usize> {
    let cfi_spine_0 = i64::from(cfi_spine_ordinal(cfi)?) - 1;
    spine_indices.iter().rposition(|s| *s <= cfi_spine_0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cfi_spine_ordinal_halves_the_even_package_step() {
        assert_eq!(cfi_spine_ordinal("epubcfi(/6/14[chap3]!/4/2/1:0)"), Some(7));
        assert_eq!(cfi_spine_ordinal("epubcfi(/6/2!/4)"), Some(1));
    }

    #[test]
    fn cfi_spine_ordinal_rejects_non_element_and_malformed_steps() {
        assert_eq!(cfi_spine_ordinal("epubcfi(/6/13!/4)"), None); // odd step
        assert_eq!(cfi_spine_ordinal("epubcfi(/6/0!/4)"), None); // zero
        assert_eq!(cfi_spine_ordinal("not-a-cfi"), None);
    }

    #[test]
    fn chapter_index_for_cfi_picks_the_chapter_by_spine_not_percent() {
        // Three chapters starting in spine items 2, 4 and 6 (0-based). A CFI in
        // spine item 4 (package step 10 → ordinal 5 → 0-based 4) resolves to the
        // chapter that *starts* at spine 4 — the middle one — regardless of how
        // close the whole-book percents of the neighbours round.
        let spines = [2, 4, 6];
        assert_eq!(
            chapter_index_for_cfi(&spines, "epubcfi(/6/10!/4/2:0)"),
            Some(1)
        );
        // A CFI past the last chapter's spine stays on the last chapter.
        assert_eq!(chapter_index_for_cfi(&spines, "epubcfi(/6/20!/4)"), Some(2));
        // A CFI before the first chapter's spine resolves to nothing (the
        // caller keeps the percent fallback / chapter 1).
        assert_eq!(chapter_index_for_cfi(&spines, "epubcfi(/6/2!/4)"), None);
    }

    #[test]
    fn chapter_index_for_cfi_resolves_to_last_of_chapters_sharing_a_spine() {
        // Two TOC entries inside one spine document (both spine_index 3): a CFI
        // in that spine lands on the later of the two — still the open document.
        let spines = [0, 3, 3, 5];
        assert_eq!(chapter_index_for_cfi(&spines, "epubcfi(/6/8!/4)"), Some(2));
    }
}
