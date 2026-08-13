//! Reader state types and signal-initialization helpers: the load-status
//! and relocate-event types, the bottom-bar label formatter, and the
//! `use_book_metadata` hook. Extracted from `BookReadPage` so the parent
//! reads as plain component wiring.

use dioxus::prelude::*;
use omnibus_shared::EbookMetadata;

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
    // True when the rendered range reaches the book's end (epub.js
    // `location.atEnd`) — the auto `Finished` trigger. `pct` can't stand in:
    // it tracks the start of the visible range, so it tops out below 100.
    #[serde(default)]
    pub(crate) at_end: bool,
    pub(crate) chapter: u32,
    pub(crate) total_chapters: u32,
    pub(crate) chapter_title: String,
}

/// Format the bottom-bar `page` and `chapter` strings from a relocate
/// event. `page`/`total_pages` are scoped to the current chapter (see
/// [`RelocateData`]). Returns `("", "")` until epub.js has produced a
/// relocation.
pub(crate) fn format_progress_labels(loc: &RelocateData) -> (String, String) {
    let page = if loc.total_pages > 0 {
        format!(
            "p.\u{a0}{} of {}\u{a0}\u{b7}\u{a0}{}%",
            loc.page, loc.total_pages, loc.pct
        )
    } else if loc.pct > 0 {
        format!("{}%", loc.pct)
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

/// Format the phone minimal-chrome footer: just the page number (or the
/// percent, before pagination resolves) — no "of total", no chapter. The
/// richer [`format_progress_labels`] readout is what the visible footer
/// shows; this is deliberately the bare number for the hidden-chrome state.
pub(crate) fn format_ambient_page(loc: &RelocateData) -> String {
    if loc.total_pages > 0 {
        loc.page.to_string()
    } else if loc.pct > 0 {
        format!("{}%", loc.pct)
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
        format!("Ch.\u{a0}{} \u{b7} {}%", loc.chapter, loc.pct)
    } else if loc.pct > 0 {
        format!("{}%", loc.pct)
    } else {
        String::new()
    }
}

/// Format the contents-drawer progress line: "184 / 272 · 68%". Empty until
/// epub.js has paginated the book.
pub(crate) fn format_contents_progress(loc: &RelocateData) -> String {
    if loc.total_pages > 0 {
        format!("{} / {} \u{b7} {}%", loc.page, loc.total_pages, loc.pct)
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
mod tests {
    use super::*;

    // First-paint contract: `BookReadPage` seeds `status` from
    // `ReaderStatus::default()` so SSR and the first WASM render produce
    // identical markup (the `rd-overlay` loading node is present in both).
    // Flipping this default would re-introduce the hydration mismatch
    // described in .claude/rules/07-hydration.md.
    #[test]
    fn reader_status_default_is_loading_for_ssr_wasm_parity() {
        assert_eq!(ReaderStatus::default(), ReaderStatus::Loading);
    }

    #[test]
    fn format_progress_labels_returns_empty_strings_before_first_relocate() {
        let (page, chapter) = format_progress_labels(&RelocateData::default());
        assert_eq!(page, "");
        assert_eq!(chapter, "");
    }

    #[test]
    fn format_progress_labels_formats_page_and_chapter_strings() {
        let data = RelocateData {
            cfi: None,
            page: 42,
            total_pages: 300,
            pct: 14,
            frac: 0.14,
            at_end: false,
            chapter: 3,
            total_chapters: 24,
            chapter_title: String::new(),
        };
        let (page, chapter) = format_progress_labels(&data);
        assert!(page.contains("p."));
        assert!(page.contains("42"));
        assert!(page.contains("300"));
        assert!(page.contains("14%"));
        assert!(chapter.contains("Ch"));
        assert!(chapter.contains("3"));
        assert!(chapter.contains("24"));
    }

    #[test]
    fn format_ambient_page_returns_bare_page_number_when_paginated() {
        let data = RelocateData {
            page: 142,
            total_pages: 300,
            pct: 47,
            ..Default::default()
        };
        assert_eq!(format_ambient_page(&data), "142");
    }

    #[test]
    fn format_ambient_page_falls_back_to_pct_then_empty() {
        let mut data = RelocateData {
            pct: 47,
            ..Default::default()
        };
        assert_eq!(format_ambient_page(&data), "47%");
        data.pct = 0;
        assert_eq!(format_ambient_page(&data), "");
    }

    #[test]
    fn format_title_sub_formats_chapter_and_pct_and_falls_back() {
        let mut data = RelocateData {
            chapter: 14,
            pct: 68,
            ..Default::default()
        };
        assert_eq!(format_title_sub(&data), "Ch.\u{a0}14 \u{b7} 68%");
        data.chapter = 0;
        assert_eq!(format_title_sub(&data), "68%");
        assert_eq!(format_title_sub(&RelocateData::default()), "");
    }

    #[test]
    fn format_contents_progress_formats_pages_and_is_empty_before_pagination() {
        let data = RelocateData {
            page: 184,
            total_pages: 272,
            pct: 68,
            ..Default::default()
        };
        assert_eq!(format_contents_progress(&data), "184 / 272 \u{b7} 68%");
        assert_eq!(format_contents_progress(&RelocateData::default()), "");
    }

    // Regression for issue #1234: a page turn must never render a stalled
    // (unchanged) figure, even across a chapter boundary where `page` resets.
    #[test]
    fn format_progress_labels_resets_page_and_total_on_crossing_a_chapter_boundary() {
        let before = RelocateData {
            page: 22,
            total_pages: 22,
            pct: 40,
            chapter: 3,
            total_chapters: 24,
            ..Default::default()
        };
        let after = RelocateData {
            page: 1,
            total_pages: 8,
            pct: 41,
            chapter: 4,
            total_chapters: 24,
            ..Default::default()
        };
        let (before_page, _) = format_progress_labels(&before);
        let (after_page, _) = format_progress_labels(&after);
        assert_eq!(before_page, "p.\u{a0}22 of 22\u{a0}\u{b7}\u{a0}40%");
        assert_eq!(after_page, "p.\u{a0}1 of 8\u{a0}\u{b7}\u{a0}41%");
        assert_ne!(
            before_page, after_page,
            "a page turn must never render a stalled (unchanged) page figure"
        );
    }

    #[test]
    fn format_progress_labels_falls_back_to_pct_only_when_total_pages_unknown() {
        let data = RelocateData {
            cfi: None,
            page: 0,
            total_pages: 0,
            pct: 7,
            frac: 0.07,
            at_end: false,
            chapter: 0,
            total_chapters: 0,
            chapter_title: String::new(),
        };
        let (page, chapter) = format_progress_labels(&data);
        assert_eq!(page, "7%");
        assert_eq!(chapter, "");
    }
}
