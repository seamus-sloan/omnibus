//! Mobile library layout for the landing page — a compact "Your shelf" surface
//! (slim header, Shelves entry, three-column cover grid) rendered by the native
//! shell instead of the web rail + toolbar. Shares [`super::LandingPage`]'s data
//! pipeline; this module owns only the mobile presentation.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::EbookMetadata;

use crate::components::atrium::Cover;
use crate::Route;

/// Props for [`MobileLanding`] — the already-derived view state handed
/// down from [`super::LandingPage`].
#[derive(Props, Clone, PartialEq)]
pub(super) struct MobileLandingProps {
    /// Total book count shown in the "N books" label.
    pub book_count: usize,
    /// The page of books to render as cover cells.
    pub books: Vec<EbookMetadata>,
    /// True while the first page is still loading.
    pub is_loading: bool,
    /// True when more pages remain to load.
    pub has_more: bool,
    /// True while a "Load more" fetch is in flight.
    pub is_loading_more: bool,
    /// Fired when the "Load more" button is pressed.
    pub on_load_more: EventHandler<()>,
    /// Base server URL used to build thumbnail `src`/`srcset`.
    pub server_url: String,
}

/// Mobile landing surface. Fed the already-derived view state from
/// [`super::LandingPage`] so the data path stays shared across targets.
#[component]
pub(super) fn MobileLanding(props: MobileLandingProps) -> Element {
    let MobileLandingProps {
        book_count,
        books,
        is_loading,
        has_more,
        is_loading_more,
        on_load_more,
        server_url,
    } = props;
    rsx! {
        div { class: "m-lib", "data-testid": "mobile-landing",
            header { class: "m-lib-head",
                div { class: "omn-brand-word m-lib-brand", "Omnibus" }
                div { class: "m-lib-head-actions",
                    // Search entry — opens the mobile-native search screen.
                    // Account/settings lives on the bottom-nav "You" tab.
                    Link {
                        to: Route::MobileSearch {},
                        class: "m-icon-btn",
                        "aria-label": "Search",
                        "data-testid": "mobile-search-entry",
                        {search_glyph()}
                    }
                }
            }

            div { class: "m-lib-title",
                span { class: "label", "{book_count} books" }
                h2 { class: "m-head-title",
                    "Your "
                    span { class: "m-em", "shelf" }
                }
            }

            Link {
                to: Route::Shelves {},
                class: "m-shelves-entry",
                "data-testid": "mobile-shelves-entry",
                span { class: "m-shelves-entry-icon", {bookmark_glyph()} }
                span { class: "m-shelves-entry-body",
                    span { class: "m-shelves-entry-name", "Shelves" }
                    span { class: "m-shelves-entry-sub", "Smart & hand-picked collections" }
                }
                span { class: "m-shelves-entry-chevron", {chevron()} }
            }

            if is_loading && books.is_empty() {
                p { class: "subtitle m-lib-loading", "Loading\u{2026}" }
            } else {
                div { class: "m-cover-grid", "data-testid": "mobile-lib-grid", role: "list",
                    for book in books.into_iter() {
                        {cover_cell(book, &server_url)}
                    }
                }
                if has_more {
                    button {
                        r#type: "button",
                        class: "btn m-load-more",
                        "data-testid": "mobile-load-more",
                        disabled: is_loading_more,
                        onclick: move |_| on_load_more.call(()),
                        if is_loading_more { "Loading\u{2026}" } else { "Load more" }
                    }
                }
            }
        }
    }
}

/// One cover cell: cover art + title + author, linking to the detail page.
/// A plain fn (rendered per book) — no hooks, so it can't perturb the parent's
/// hook order.
fn cover_cell(book: EbookMetadata, server_url: &str) -> Element {
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let title = book.title.as_deref().unwrap_or(&book.filename).to_string();
    let author = book
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    let (src, srcset) = thumb_srcs(&book, &uuid, server_url);

    rsx! {
        Link {
            key: "{uuid}",
            to: Route::BookDetail { uuid: uuid.clone() },
            class: "m-cover-cell",
            role: "listitem",
            "data-testid": "mobile-lib-tile",
            "aria-label": "Open details for {title}",
            Cover {
                book,
                src_override: src,
                srcset,
                sizes: Some("33vw".to_string()),
            }
            div { class: "m-cover-cell-title", "{title}" }
            if !author.is_empty() {
                div { class: "m-cover-cell-author", "{author}" }
            }
        }
    }
}

/// Responsive thumbnail `src`/`srcset` for a book (mirrors the web grid).
fn thumb_srcs(
    book: &EbookMetadata,
    uuid: &str,
    server_url: &str,
) -> (Option<String>, Option<String>) {
    if book.cover_url.is_some() {
        // `thumb_url` appends the mobile `?token=` an `<img>` fetch needs (see `contexts::media_url`).
        let sm = crate::thumb_url(server_url, uuid, "sm");
        let md = crate::thumb_url(server_url, uuid, "md");
        let lg = crate::thumb_url(server_url, uuid, "lg");
        (
            Some(md.clone()),
            Some(format!("{sm} 160w, {md} 320w, {lg} 640w")),
        )
    } else {
        (None, None)
    }
}

fn search_glyph() -> Element {
    rsx! {
        svg {
            width: "18", height: "18", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            circle { cx: "11", cy: "11", r: "8" }
            line { x1: "21", y1: "21", x2: "16.65", y2: "16.65" }
        }
    }
}

fn bookmark_glyph() -> Element {
    rsx! {
        svg {
            width: "18", height: "18", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "1.8", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M19 21l-7-5-7 5V5a2 2 0 0 1 2-2h10a2 2 0 0 1 2 2z" }
        }
    }
}

fn chevron() -> Element {
    rsx! {
        svg {
            width: "16", height: "16", view_box: "0 0 24 24", fill: "none",
            stroke: "currentColor", stroke_width: "2", stroke_linecap: "round", stroke_linejoin: "round",
            path { d: "M9 18l6-6-6-6" }
        }
    }
}
