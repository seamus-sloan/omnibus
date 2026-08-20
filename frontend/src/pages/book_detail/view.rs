//! Loaded-book view composition for [`super::BookDetailPage`].
//!
//! Derives display fields from the fetched [`EbookMetadata`] (breadcrumbs,
//! kicker, series label, format flags) and renders the platform body: web's
//! hero + rail + physical panel + suggestions grid, or mobile's single-column
//! re-flow via [`super::mobile`].

use dioxus::prelude::*;
// Only the web branch of `render_loaded` renders the "Back to library" Link;
// mobile re-flows through `mobile.rs`, so an unconditional import is unused there.
#[cfg(not(feature = "mobile"))]
use dioxus_router::Link;
use omnibus_shared::{EbookMetadata, SuggestionsResponse};

use crate::Route;

#[cfg(not(feature = "mobile"))]
use super::body::{BdAuthorCluster, BdBodyMain, BdPageCtx, BdRailSection};
#[cfg(not(feature = "mobile"))]
use super::hero::{self, BdHeroSection};
#[cfg(feature = "mobile")]
use super::mobile;
#[cfg(not(feature = "mobile"))]
use super::physical;
#[cfg(not(feature = "mobile"))]
use super::sync_link;
use super::{BdCrumbItem, DescriptionSignals, PhysSignals};

fn kicker_label(year: &str) -> String {
    if year.is_empty() {
        "Book".to_string()
    } else {
        format!("Book · {year}")
    }
}

fn series_label(series: Option<&str>, index: Option<&str>) -> Option<String> {
    match (series, index) {
        (Some(s), Some(i)) => Some(format!("{s} #{i}")),
        (Some(s), None) => Some(s.to_string()),
        _ => None,
    }
}

/// Build the breadcrumb item list for a loaded book.
fn build_crumbs(
    b: &EbookMetadata,
    title: &str,
    primary_author: &str,
    series_label: &Option<String>,
) -> Vec<BdCrumbItem> {
    let mut crumbs = vec![BdCrumbItem {
        text: "Home".to_string(),
        target: Some(Route::Landing {}),
    }];
    if !primary_author.is_empty() {
        let author_route = b
            .creators
            .first()
            .and_then(|c| c.id)
            .map(|id| Route::AuthorDetail { id });
        crumbs.push(BdCrumbItem {
            text: primary_author.to_string(),
            target: author_route,
        });
    }
    if let Some(label) = series_label.clone() {
        let series_route = b.series_id.map(|id| Route::SeriesDetail { id });
        crumbs.push(BdCrumbItem {
            text: label,
            target: series_route,
        });
    }
    crumbs.push(BdCrumbItem {
        text: title.to_string(),
        target: None,
    });
    crumbs
}

/// Pre-derived strings + flags ready to feed the loaded-book sections.
/// Split out of [`render_loaded`] so the rsx body stays a thin composition
/// of named sub-components. Mobile reads a subset (no breadcrumb / author-id
/// cluster), so some fields are unused there.
#[cfg_attr(feature = "mobile", allow(dead_code))]
pub(super) struct LoadedBookView {
    pub(super) title: String,
    pub(super) primary_author: String,
    pub(super) author_id: Option<i64>,
    pub(super) authors_line: String,
    pub(super) kicker: String,
    pub(super) series: Option<String>,
    pub(super) accent_style: String,
    pub(super) has_audio: bool,
    pub(super) has_ebook: bool,
    pub(super) has_comic: bool,
    pub(super) crumbs: Vec<BdCrumbItem>,
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
    let year = b
        .published
        .as_deref()
        .and_then(|p| p.get(0..4))
        .unwrap_or("")
        .to_string();
    let kicker = kicker_label(&year);
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
    let crumbs = build_crumbs(b, &title, &primary_author, &series);
    LoadedBookView {
        title,
        primary_author,
        author_id,
        authors_line,
        kicker,
        series,
        accent_style,
        has_audio,
        has_ebook,
        has_comic,
        crumbs,
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

/// Render the fully-loaded book detail view — web hero + rail + physical
/// panel + suggestions grid.
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
    // Web keeps its own local copy inside `BdTitleCol` (hero.rs), a real
    // `#[component]` that gets a fresh scope per mount — no hook-order risk
    // there, so this param is unused on this target.
    let _ = description;
    let RailButtons {
        merge: merge_button,
        delete: delete_button,
    } = rail;
    let LoadedBookView {
        title,
        primary_author,
        author_id,
        authors_line,
        kicker,
        series,
        accent_style,
        has_audio,
        has_ebook,
        has_comic,
        crumbs,
    } = derive_loaded_view(&b);

    let uuid = b.unique_identifier.clone().unwrap_or_default();
    // Extract the physical-panel inputs before `b` is moved into the rail.
    let is_fileless = b.formats.is_empty();
    let accent = b.accent.clone();
    // The "Your progress" block lives inside the hero's title column (below
    // the CTA row); it only exists for dual-format books.
    let sync_panel = if has_ebook && has_audio {
        rsx! {
            sync_link::BdSyncPanel {
                uuid: uuid.clone(),
                refresh,
                after_merge,
            }
        }
    } else {
        rsx! {}
    };
    rsx! {
        div { class: "bd-root", style: "{accent_style}",
            BdHeroSection {
                b: b.clone(),
                title: title.clone(),
                chrome: hero::BdHeroChrome { kicker, crumbs },
                avail: hero::Availability {
                    has_ebook,
                    has_audio,
                    has_comic,
                },
                phys,
                sync_panel,
            }
            physical::BdPhysicalPanel {
                uuid: uuid.clone(),
                is_fileless,
                refresh,
                phys,
            }
            section { class: "bd-body-grid",
                BdBodyMain {
                    uuid: uuid.clone(),
                    title: title.clone(),
                    author: BdAuthorCluster { primary_author, author_id, author_books },
                    accent,
                    suggestions,
                    ctx: BdPageCtx { server_url, is_admin },
                }
                BdRailSection {
                    b,
                    title,
                    authors_line,
                    series,
                    merge_button,
                    delete_button,
                }
            }
            div { class: "bd-footer",
                Link { to: Route::Landing {}, class: "btn", "Back to library" }
            }
        }
    }
}

#[cfg(test)]
mod tests;
