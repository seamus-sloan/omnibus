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
    let book_for_cover = book.clone();
    let nav = use_navigator();

    // Prefer the responsive `/api/thumbs/:uuid/{sm,md,lg}` endpoint over
    // the raw `/api/covers/:uuid`: smaller payload (WebP, resized per
    // slot) and the URL is server-prefixed so mobile picks up the right
    // origin. Books with no cover fall back to the Atrium plate
    // template.
    let (thumb_src, thumb_srcset) = if book.cover_url.is_some() {
        let base = format!("{server_url}/api/thumbs/{uuid}");
        (
            Some(format!("{base}/md")),
            Some(format!("{base}/sm 160w, {base}/md 320w, {base}/lg 640w")),
        )
    } else {
        (None, None)
    };

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
                book: book_for_cover,
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
