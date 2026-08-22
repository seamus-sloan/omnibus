//! Reader state types and signal-initialization helpers: the load-status
//! and relocate-event types, the bottom-bar label formatter, and the
//! `use_book_metadata` hook. Extracted from `BookReadPage` so the parent
//! reads as plain component wiring.

use dioxus::prelude::*;
use omnibus_shared::EbookMetadata;

use super::toc_drawer::TocEntry;
use crate::contexts::use_server_url;
use crate::data;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
pub(crate) enum ReaderStatus {
    // INVARIANT: `Loading` is the SSR/WASM-first-paint seed (see
    // `BookReadPage`). Changing the default flips the rendered overlay and
    // breaks hydration — see .claude/rules/07-hydration.md.
    #[default]
    Loading,
    Ready,
    Failed,
    /// Mobile-only outcome: offline with no completed local download, so the
    /// EPUB stream was never attempted (epub.js against a dead server hangs
    /// on "Loading…" forever — the WebView fetch is outside our timeouts).
    /// Web renders the overlay arm but never constructs the variant.
    #[cfg_attr(not(feature = "mobile"), allow(dead_code))]
    Offline,
}

/// Relocated event data from epub.js glue (deserialized from JSON).
#[derive(Clone, Default, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RelocateData {
    // The web + mobile progress-save paths read `cfi`; the other fields are read unconditionally by the bottom-bar render.
    #[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
    pub(crate) cfi: Option<String>,
    // Visual page within the *current spine section* (epub.js
    // `location.start.displayed`), not a whole-book count: it increments by
    // one per turn within a chapter, and resets to 1 on crossing into the
    // next chapter. `pct` below is the whole-book position.
    pub(crate) page: u32,
    pub(crate) total_pages: u32,
    pub(crate) pct: u32,
    /// Full-precision twin of `pct` (0.0..=1.0) — the "synced here"
    /// declaration records this, not the rounded display value. Read only
    /// by the web pill; native iOS owns the gesture on mobile.
    #[serde(default)]
    #[cfg_attr(feature = "mobile", allow(dead_code))]
    pub(crate) frac: f64,
    // True while `pct` is the glue's coarse spine-derived approximation —
    // the whole-book locations map hasn't resolved yet (generation runs in
    // the background after first paint; issue #1896). The formatters render
    // an approximate percent as "~N%" so a first session never shows a
    // frozen, falsely-precise "0%".
    #[serde(default)]
    pub(crate) pct_approx: bool,
    // True when the rendered range reaches the book's end (epub.js
    // `location.atEnd`) — the auto `Finished` trigger. `pct` can't stand in:
    // it tracks the start of the visible range, so it tops out below 100.
    #[serde(default)]
    pub(crate) at_end: bool,
    pub(crate) chapter: u32,
    pub(crate) total_chapters: u32,
    pub(crate) chapter_title: String,
    /// True when the glue re-states a position the host already knows (the
    /// restore settle, the locations-resolved re-emit) rather than reporting
    /// movement. Rendered like any relocate, but never persisted: an echo
    /// write stamps a fresh clock on an unmoved position, which out-orders a
    /// newer counterpart-format position at the cross-format clock gate.
    #[serde(default)]
    #[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
    pub(crate) echo: bool,
}

/// The percent readout: "N%" once the whole-book locations map has
/// resolved, "~N%" while `pct` is still the coarse spine approximation —
/// honest instead of falsely precise (issue #1896).
fn pct_label(loc: &RelocateData) -> String {
    if loc.pct_approx {
        format!("~{}%", loc.pct)
    } else {
        format!("{}%", loc.pct)
    }
}

/// Format the bottom-bar `page` and `chapter` strings from a relocate
/// event. `page`/`total_pages` are scoped to the current chapter (see
/// [`RelocateData`]). Returns `("", "")` until epub.js has produced a
/// relocation.
pub(crate) fn format_progress_labels(loc: &RelocateData) -> (String, String) {
    let page = if loc.total_pages > 0 {
        format!(
            "p.\u{a0}{} of {}\u{a0}\u{b7}\u{a0}{}",
            loc.page,
            loc.total_pages,
            pct_label(loc)
        )
    } else if loc.pct > 0 {
        pct_label(loc)
    } else {
        String::new()
    };
    let chapter = if loc.total_chapters > 0 {
        format!("Ch\u{a0}{} of {}", loc.chapter, loc.total_chapters)
    } else {
        String::new()
    };
    (page, chapter)
}

/// Resolve the displayed chapter index/total from the flat TOC's own array
/// order rather than the glue's `chapter`/`total_chapters` pair verbatim,
/// carrying the previous chapter forward instead of regressing to 0 when
/// the incoming title doesn't match any TOC entry.
#[cfg_attr(not(any(feature = "web", feature = "mobile")), allow(dead_code))]
pub(crate) fn resolve_chapter_position(
    toc: &[TocEntry],
    incoming: &RelocateData,
    previous_chapter: u32,
) -> (u32, u32) {
    if toc.is_empty() {
        return (incoming.chapter, incoming.total_chapters);
    }
    let total = toc.len() as u32;
    let chapter = if incoming.chapter_title.is_empty() {
        previous_chapter
    } else {
        toc.iter()
            .position(|entry| entry.label == incoming.chapter_title)
            .map_or(previous_chapter, |idx| idx as u32 + 1)
    };
    (chapter, total)
}

/// Format the phone minimal-chrome footer: just the page number (or the
/// percent, before pagination resolves) — no "of total", no chapter. The
/// richer [`format_progress_labels`] readout is what the visible footer
/// shows; this is deliberately the bare number for the hidden-chrome state.
pub(crate) fn format_ambient_page(loc: &RelocateData) -> String {
    if loc.total_pages > 0 {
        loc.page.to_string()
    } else if loc.pct > 0 {
        pct_label(loc)
    } else {
        String::new()
    }
}

/// Format the phone top-bar sub-line under the book title: "Ch. 3 · 14%".
/// Falls back to the percent alone before the TOC resolves; empty until
/// epub.js has produced a relocation. Rendered on every target (rule 07);
/// only the phone breakpoint displays it.
pub(crate) fn format_title_sub(loc: &RelocateData) -> String {
    if loc.chapter > 0 {
        format!("Ch.\u{a0}{} \u{b7} {}", loc.chapter, pct_label(loc))
    } else if loc.pct > 0 {
        pct_label(loc)
    } else {
        String::new()
    }
}

/// Format the contents-drawer progress line: "184 / 272 · 68%". Empty until
/// epub.js has paginated the book.
pub(crate) fn format_contents_progress(loc: &RelocateData) -> String {
    if loc.total_pages > 0 {
        format!(
            "{} / {} \u{b7} {}",
            loc.page,
            loc.total_pages,
            pct_label(loc)
        )
    } else {
        String::new()
    }
}

/// Drop the previous book's title and kick off a fresh `get_ebook` fetch
/// whenever `uuid` changes. SPA navigations between books would otherwise
/// flash the previous title while the request is in flight, and an epoch
/// guard keeps a slower, superseded fetch from overwriting a newer one.
pub(crate) fn use_book_metadata(uuid: String) -> Signal<Option<EbookMetadata>> {
    let mut book_meta: Signal<Option<EbookMetadata>> = use_signal(|| None);
    let server_url = use_server_url();
    // Epoch guard so a fetch for a previously-viewed book can't overwrite
    // `book_meta` after a newer navigation has already superseded it
    // (mirrors book_detail's `suggestions_epoch`).
    let mut fetch_epoch = use_signal(|| 0u64);
    use_effect(use_reactive!(|uuid| {
        book_meta.set(None);
        let url = server_url.clone();
        let uuid = uuid.clone();
        let epoch = {
            fetch_epoch.with_mut(|e| *e += 1);
            *fetch_epoch.peek()
        };
        spawn(async move {
            if let Ok(Some(b)) = data::get_ebook(&url, &uuid).await {
                if *fetch_epoch.peek() == epoch {
                    book_meta.set(Some(b));
                }
            }
        });
    }));
    book_meta
}

#[cfg(all(test, not(any(feature = "web", feature = "mobile"))))]
mod tests;
