//! Loaded-book view composition for [`super::BookDetailPage`]. Derives display
//! fields from the fetched [`EbookMetadata`] (series label, format flags,
//! accent) and renders the platform body: web's W4 marquee stage via
//! [`super::w4`], or mobile's single-column re-flow via [`super::mobile`].

use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, SuggestionsResponse};

#[cfg(feature = "mobile")]
use super::mobile;
#[cfg(not(feature = "mobile"))]
use super::w4::{W4AdminActions, W4Stage, W4StageCtx, W4ViewFacts};
use super::{DescriptionSignals, PhysSignals};

fn series_label(series: Option<&str>, index: Option<&str>) -> Option<String> {
    match (series, index) {
        (Some(s), Some(i)) => Some(format!("{s} #{i}")),
        (Some(s), None) => Some(s.to_string()),
        _ => None,
    }
}

/// Pre-derived strings + flags ready to feed the loaded-book sections.
/// Split out of [`render_loaded`] so the rsx body stays a thin composition
/// of named sub-components. Mobile reads a subset (no author-id cluster), so
/// some fields are unused there.
#[cfg_attr(feature = "mobile", allow(dead_code))]
pub(super) struct LoadedBookView {
    pub(super) title: String,
    pub(super) primary_author: String,
    pub(super) author_id: Option<i64>,
    pub(super) authors_line: String,
    pub(super) series: Option<String>,
    pub(super) accent_style: String,
    pub(super) has_audio: bool,
    pub(super) has_ebook: bool,
    pub(super) has_comic: bool,
}

/// Compute the per-section display fields from the loaded book.
pub(super) fn derive_loaded_view(b: &EbookMetadata) -> LoadedBookView {
    let title = b.display_title();
    let primary_author = b
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let author_id = b.creators.first().and_then(|c| c.id);
    let authors_line = b
        .creators
        .iter()
        .map(|c| c.name.clone())
        .collect::<Vec<_>>()
        .join(", ");
    let series = series_label(b.series.as_deref(), b.series_index.as_deref());
    let accent_style = b
        .accent
        .as_deref()
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();
    // M4A is a first-class indexed audio format (`db::audiobook`); leaving
    // it out here made the landing hero's link invite dead-end on a detail
    // page with no sync affordance for EPUB+M4A books.
    let has_audio = b.formats.iter().any(|f| {
        f.eq_ignore_ascii_case("m4b")
            || f.eq_ignore_ascii_case("m4a")
            || f.eq_ignore_ascii_case("mp3")
    });
    let has_ebook = b
        .formats
        .iter()
        .any(|f| f.eq_ignore_ascii_case("epub") || f.eq_ignore_ascii_case("pdf"));
    let has_comic = b.formats.iter().any(|f| f.eq_ignore_ascii_case("cbz"));
    LoadedBookView {
        title,
        primary_author,
        author_id,
        authors_line,
        series,
        accent_style,
        has_audio,
        has_ebook,
        has_comic,
    }
}

/// Prebuilt web-only rail action buttons (Merge / Delete), threaded from
/// [`super::render_book_shell`] into [`render_loaded`]. Grouped only to keep
/// that call site under clippy's argument cap; both are always `None` on
/// mobile, and mobile discards the whole struct rather than reading its
/// fields (see [`LoadedBookView`] for the same pattern).
#[cfg_attr(feature = "mobile", allow(dead_code))]
pub(super) struct RailButtons {
    pub(super) merge: Option<Element>,
    pub(super) delete: Option<Element>,
}

/// Server URL + admin flag + book-refetch signal threaded into
/// [`render_loaded`]. Grouped so the call site stays under clippy's argument
/// cap; `Copy`-free (`server_url` is a `String`), so it's passed by value once.
pub(super) struct LoadedCtx {
    pub(super) server_url: String,
    pub(super) is_admin: bool,
    pub(super) refresh: Signal<u32>,
    pub(super) phys: PhysSignals,
    /// One-shot flag from the merge dialog: a merge that just produced a
    /// dual-format book auto-opens the alignment modal.
    pub(super) after_merge: Signal<bool>,
}

/// Render the fully-loaded book detail view — mobile re-flow into a single
/// column via [`mobile::render_loaded_mobile`]. Merge/delete and the
/// physical/wishlist rail stay web-only, so `rail`/`refresh`/`phys` are
/// unused here.
#[cfg(feature = "mobile")]
pub(super) fn render_loaded(
    b: EbookMetadata,
    description: DescriptionSignals,
    author_books: Vec<EbookMetadata>,
    rail: RailButtons,
    suggestions: Option<SuggestionsResponse>,
    ctx: LoadedCtx,
) -> Element {
    let LoadedCtx {
        server_url,
        is_admin,
        refresh,
        phys,
        after_merge,
    } = ctx;
    let _ = rail;
    let _ = (refresh, phys, after_merge);
    mobile::render_loaded_mobile(mobile::MobileBookView {
        b,
        author_books,
        suggestions,
        is_admin,
        server_url,
        description,
    })
}

/// Render the fully-loaded book detail view — the web W4 marquee stage
/// (cover pinned left, seven snap-scrolled stops on the right).
#[cfg(not(feature = "mobile"))]
pub(super) fn render_loaded(
    b: EbookMetadata,
    description: DescriptionSignals,
    author_books: Vec<EbookMetadata>,
    rail: RailButtons,
    suggestions: Option<SuggestionsResponse>,
    ctx: LoadedCtx,
) -> Element {
    let LoadedCtx {
        server_url,
        is_admin,
        refresh,
        phys,
        after_merge,
    } = ctx;
    // Web keeps its own local copy inside `W4HomeStop`, a real `#[component]`
    // that gets a fresh scope per mount — no hook-order risk there, so this
    // param is unused on this target.
    let _ = description;
    let RailButtons {
        merge: merge_button,
        delete: delete_button,
    } = rail;
    let loaded = derive_loaded_view(&b);
    let accent_style = loaded.accent_style.clone();
    let view = W4ViewFacts::from_loaded(&loaded);

    rsx! {
        div { class: "bd-root bdw4", style: "{accent_style}",
            W4Stage {
                b,
                view,
                author_books,
                suggestions,
                admin: W4AdminActions {
                    merge_button,
                    delete_button,
                },
                phys,
                ctx: W4StageCtx {
                    server_url,
                    is_admin,
                    refresh,
                    after_merge,
                },
            }
        }
    }
}

#[cfg(test)]
mod tests;
