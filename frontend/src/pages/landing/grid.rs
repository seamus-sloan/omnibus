//! Cover-grid view for the landing page.
//!
//! Renders the hydrated book list as Atrium `Cover` tiles, linking each to
//! the book-detail page. Used by [`super::LandingPage`] when the view-mode
//! toggle is set to grid.

use dioxus::prelude::*;
use dioxus_router::use_navigator;
use omnibus_shared::EbookMetadata;

use super::sorting::{contributor_names, row_ident};
use crate::Route;

/// Entrance-cascade delay for tile `index`, mirroring the iOS settle cascade:
/// 40 ms steps, modulo 8 so late pages animate like the first.
pub(super) fn stagger_ms(index: usize) -> usize {
    (index % 8) * 40
}

#[component]
pub(super) fn BookGrid(books: Vec<EbookMetadata>, server_url: String) -> Element {
    rsx! {
        div { class: "lib-grid", "data-testid": "lib-grid", role: "list",
            for (index, book) in books.into_iter().enumerate() {
                GridTile {
                    key: "{row_ident(&book)}",
                    book: book,
                    server_url: server_url.clone(),
                    index,
                }
            }
        }
    }
}

#[component]
fn GridTile(book: EbookMetadata, server_url: String, index: usize) -> Element {
    // Stable per-book uuid drives both detail-route URL and thumb URL
    // (see `Route::BookDetail`).
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let display_title = book.title.as_deref().unwrap_or(&book.filename).to_string();
    let tile_testid = format!("ebook-tile-{}", row_ident(&book));
    let authors = contributor_names(&book.creators);
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
            style: "animation-delay: {stagger_ms(index)}ms",
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
            div { class: "lib-tile-title", "{display_title}" }
            if !authors.is_empty() {
                div { class: "lib-tile-author", "{authors}" }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::stagger_ms;

    #[test]
    fn stagger_ms_steps_by_40_and_wraps_every_eight_tiles() {
        assert_eq!(stagger_ms(0), 0);
        assert_eq!(stagger_ms(3), 120);
        assert_eq!(stagger_ms(7), 280);
        assert_eq!(stagger_ms(8), 0);
        assert_eq!(stagger_ms(19), 120);
    }
}
