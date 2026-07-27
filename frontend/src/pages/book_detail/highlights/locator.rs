//! Derive a human-readable locator from an EPUB CFI. A highlight stores only
//! its CFI range, and resolving that to a page needs the paginated book, so
//! the best the detail page can name without opening the EPUB is which spine
//! item (section) the passage sits in.

/// Human locator for a highlight's CFI, e.g. `"Section 7"`. `None` when the
/// string isn't a CFI we can read a spine step out of — the caller then shows
/// the saved date alone rather than a made-up position.
pub(super) fn highlight_locator(cfi: &str) -> Option<String> {
    spine_position(cfi).map(|n| format!("Section {n}"))
}

/// 1-based spine index encoded in a CFI's pre-`!` package step.
///
/// A CFI opens with the path through the package document — `/6/14[chap3]!…`
/// — where `/6` selects the `<spine>` and the step after it is the *child*
/// index of the spine item. Child indices are even (odd steps address text
/// nodes), so the item's ordinal is that step halved.
fn spine_position(cfi: &str) -> Option<u32> {
    let inner = cfi.trim().strip_prefix("epubcfi(")?.strip_suffix(')')?;
    // Everything up to the first `!` is the package-document path; what
    // follows steps inside the spine item itself.
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
