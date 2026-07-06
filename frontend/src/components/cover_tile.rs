//! Shared cover-tile primitives: the responsive `src`/`srcset` builder and
//! the `CoverTile` wrapper used by the landing grid, shelf detail, and the
//! create-shelf / add-books pickers. One home for the tile chrome so a
//! styling tweak lands in every surface at once.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::EbookMetadata;

use crate::components::atrium::Cover;
use crate::Route;

/// Responsive `md`-src + sm/md/lg `srcset` for a book's cover thumbnail; `(None, None)` when it has no cover.
pub fn thumb_srcs(
    book: &EbookMetadata,
    uuid: &str,
    server_url: &str,
) -> (Option<String>, Option<String>) {
    if book.cover_url.is_some() {
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

/// Toggle a book `uuid` in a picker's selection signal (remove if present, else append).
pub fn toggle_picked(picked: &mut Signal<Vec<String>>, uuid: &str) {
    picked.with_mut(|v| {
        if let Some(pos) = v.iter().position(|x| x == uuid) {
            v.remove(pos);
        } else {
            v.push(uuid.to_string());
        }
    });
}

/// How a [`CoverTile`] wraps its [`Cover`].
#[derive(Clone, PartialEq)]
pub enum CoverTileKind {
    /// A `<div class="shelf-cover-tile">` with no interaction — the smart-rule
    /// preview grid.
    ReadOnly,
    /// A selectable `<button class="shelf-cover-tile shelf-cover-tile--selectable">`
    /// carrying `aria-pressed`; the picker grids. `selected` drives the
    /// `--picked` modifier and `on_toggle` fires on click.
    Selectable {
        selected: bool,
        on_toggle: EventHandler<()>,
    },
    /// A `<Link>` to the book-detail page with a title caption below — the
    /// shelf member grid.
    MemberLink { title: String },
}

/// One cover tile; `kind` selects the wrapper chrome around a shared [`Cover`].
#[component]
pub fn CoverTile(
    book: EbookMetadata,
    server_url: String,
    sizes: String,
    kind: CoverTileKind,
) -> Element {
    let uuid = book.unique_identifier.clone().unwrap_or_default();
    let (src, srcset) = thumb_srcs(&book, &uuid, &server_url);
    let cover_sizes = Some(sizes);

    match kind {
        CoverTileKind::ReadOnly => rsx! {
            div { class: "shelf-cover-tile",
                Cover { book, src_override: src, srcset, sizes: cover_sizes }
            }
        },
        CoverTileKind::Selectable {
            selected,
            on_toggle,
        } => {
            let class = if selected {
                "shelf-cover-tile shelf-cover-tile--selectable shelf-cover-tile--picked"
            } else {
                "shelf-cover-tile shelf-cover-tile--selectable"
            };
            rsx! {
                button {
                    r#type: "button",
                    class: "{class}",
                    "aria-pressed": if selected { "true" } else { "false" },
                    onclick: move |_| on_toggle.call(()),
                    Cover { book, src_override: src, srcset, sizes: cover_sizes }
                }
            }
        }
        CoverTileKind::MemberLink { title } => rsx! {
            Link {
                to: Route::BookDetail { uuid: uuid.clone() },
                class: "cover-link lib-tile",
                role: "listitem",
                "data-testid": "shelf-tile",
                aria_label: "Open details for {title}",
                Cover { book, src_override: src, srcset, sizes: cover_sizes }
                div { class: "lib-tile-title", "{title}" }
            }
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn book_with_cover(cover: Option<&str>) -> EbookMetadata {
        EbookMetadata {
            cover_url: cover.map(str::to_string),
            ..Default::default()
        }
    }

    #[test]
    fn thumb_srcs_builds_three_candidate_srcset_when_cover_present() {
        let book = book_with_cover(Some("/api/covers/abc"));
        let (src, srcset) = thumb_srcs(&book, "abc", "");
        assert_eq!(src.as_deref(), Some("/api/thumbs/abc/md"));
        assert_eq!(
            srcset.as_deref(),
            Some("/api/thumbs/abc/sm 160w, /api/thumbs/abc/md 320w, /api/thumbs/abc/lg 640w"),
        );
    }

    #[test]
    fn thumb_srcs_returns_none_when_no_cover() {
        let book = book_with_cover(None);
        assert_eq!(thumb_srcs(&book, "abc", ""), (None, None));
    }
}
