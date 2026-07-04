//! Mobile library layout for the landing page — a compact "Your shelf" surface
//! (slim header, Shelves entry, three-column cover grid) rendered by the native
//! shell instead of the web rail + toolbar. Shares [`super::LandingPage`]'s data
//! pipeline; this module owns only the mobile presentation.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::EbookMetadata;

use crate::components::atrium::Cover;
use crate::Route;

/// Mobile landing surface. Fed the already-derived view state from
/// [`super::LandingPage`] so the data path stays shared across targets.
#[component]
pub(super) fn MobileLanding(
    book_count: usize,
    books: Vec<EbookMetadata>,
    is_loading: bool,
    has_more: bool,
    is_loading_more: bool,
    on_load_more: EventHandler<()>,
    server_url: String,
) -> Element {
    rsx! {
        div { class: "m-lib", "data-testid": "mobile-landing",
            header { class: "m-lib-head",
                div { class: "omn-brand-word m-lib-brand", "Omnibus" }
                div { class: "m-lib-head-actions",
                    // A dedicated mobile search screen is a follow-up; until it
                    // exists there's no valid destination (`/search/:query`
                    // needs a query), so the header carries only the avatar.
                    Link {
                        to: Route::Settings {},
                        class: "omn-avatar m-lib-avatar",
                        "aria-label": "Your account",
                        "ek"
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
        let base = format!("{server_url}/api/thumbs/{uuid}");
        (
            Some(format!("{base}/md")),
            Some(format!("{base}/sm 160w, {base}/md 320w, {base}/lg 640w")),
        )
    } else {
        (None, None)
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
