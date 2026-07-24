//! Cover-grid view for the landing page.
//!
//! Renders the hydrated book list as Atrium `Cover` tiles, linking each to
//! the book-detail page. Used by [`super::LandingPage`] when the view-mode
//! toggle is set to grid.

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::EbookMetadata;

use super::sorting::{contributor_names, row_slug};
use crate::Route;

#[component]
pub(super) fn BookGrid(books: Vec<EbookMetadata>, server_url: String) -> Element {
    rsx! {
        div { class: "lib-grid", "data-testid": "lib-grid", role: "list",
            for book in books.into_iter() {
                GridTile {
                    key: "{book.filename}",
                    book: book,
                    server_url: server_url.clone(),
                }
            }
        }
    }
}

#[component]
fn GridTile(book: EbookMetadata, server_url: String) -> Element {
    // Stable per-book uuid drives both detail-route URL and thumb URL
    // (see `Route::BookDetail`).
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let display_title = book.title.as_deref().unwrap_or(&book.filename).to_string();
    let tile_testid = format!("ebook-tile-{}", row_slug(&book.filename));
    let authors = contributor_names(&book.creators);
    // Read before `book` moves into `Cover`. Mirrors the table's formats cell,
    // which carries the same badge (#1181).
    let has_physical = book.has_physical;
    let nav = use_navigator();

    // Prefer the responsive `/api/thumbs/:uuid/{sm,md,lg}` endpoint over the
    // raw `/api/covers/:uuid`: smaller payload (WebP, resized per slot). Books
    // with no cover fall back to the Atrium plate template. Cache-busted per
    // `CoverCacheBust` so a cover edit is visible immediately on return to
    // the grid (issue #1087) rather than after the browser's 1-day thumb cache expires.
    let cover_bust =
        crate::contexts::cover_bust_for(crate::contexts::use_cover_cache_bust().0, &uuid);
    let (thumb_src, thumb_srcset) =
        crate::components::cover_tile::thumb_srcs(&book, &uuid, &server_url, cover_bust);

    let uuid_click = uuid.clone();
    let uuid_key = uuid.clone();

    rsx! {
        a {
            class: "cover-link lib-tile",
            "data-testid": "{tile_testid}",
            role: "listitem",
            tabindex: "0",
            aria_label: "Open details for {display_title}",
            onclick: move |_| { nav.push(Route::BookDetail { uuid: uuid_click.clone() }); },
            onkeydown: move |evt: Event<KeyboardData>| {
                let key = evt.key();
                if key == Key::Enter || key == Key::Character(" ".to_string()) {
                    evt.prevent_default();
                    nav.push(Route::BookDetail { uuid: uuid_key.clone() });
                }
            },
            crate::components::atrium::Cover {
                book,
                src_override: thumb_src,
                srcset: thumb_srcset,
                sizes: Some(
                    "(max-width: 640px) 160px, (max-width: 1280px) 200px, 240px"
                        .to_string(),
                ),
            }
            if has_physical {
                span {
                    class: "lib-tile-phys format-badge format-badge--physical",
                    "data-testid": "format-badge-physical",
                    title: "In your physical collection",
                    "PHYS"
                }
            }
            div { class: "lib-tile-title", "{display_title}" }
            if !authors.is_empty() {
                div { class: "lib-tile-author", "{authors}" }
            }
        }
    }
}
