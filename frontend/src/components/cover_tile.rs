//! Shared cover-tile primitives: the responsive `src`/`srcset` builder and
//! the `CoverTile` wrapper used by the landing grid, shelf detail, and the
//! create-shelf / add-books pickers. One home for the tile chrome so a
//! styling tweak lands in every surface at once.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::EbookMetadata;

use crate::components::atrium::Cover;
use crate::contexts::append_cache_bust;
use crate::Route;

/// Responsive `md`-src + sm/md/lg `srcset` for a book's cover thumbnail;
/// `(None, None)` when it has no cover. `bust` is this book's
/// [`crate::contexts::CoverCacheBust`] counter (0 = unchanged this
/// session) — `/api/thumbs/*` is cached `private, max-age=86400`, so
/// without it the grid/table would keep showing a pre-edit thumbnail for
/// the otherwise-unchanged URL after navigating back from a cover edit
/// (issue #1087).
pub fn thumb_srcs(
    book: &EbookMetadata,
    uuid: &str,
    server_url: &str,
    bust: u32,
) -> (Option<String>, Option<String>) {
    if book.cover_url.is_some() {
        let sm = append_cache_bust(crate::thumb_url(server_url, uuid, "sm"), bust);
        let md = append_cache_bust(crate::thumb_url(server_url, uuid, "md"), bust);
        let lg = append_cache_bust(crate::thumb_url(server_url, uuid, "lg"), bust);
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
    let bust = crate::contexts::cover_bust_for(crate::contexts::use_cover_cache_bust().0, &uuid);
    let (src, srcset) = thumb_srcs(&book, &uuid, &server_url, bust);
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
        let (src, srcset) = thumb_srcs(&book, "abc", "", 0);
        assert_eq!(src.as_deref(), Some("/api/thumbs/abc/md"));
        assert_eq!(
            srcset.as_deref(),
            Some("/api/thumbs/abc/sm 160w, /api/thumbs/abc/md 320w, /api/thumbs/abc/lg 640w"),
        );
    }

    #[test]
    fn thumb_srcs_returns_none_when_no_cover() {
        let book = book_with_cover(None);
        assert_eq!(thumb_srcs(&book, "abc", "", 0), (None, None));
    }

    // Regression for #1087: a nonzero cache-bust counter appends `?v=N` to
    // every srcset candidate so a browser that cached the pre-edit
    // thumbnail bytes for this URL (`Cache-Control: private,
    // max-age=86400`) is forced to re-fetch after a cover change, instead
    // of showing stale art until a manual refresh.
    #[test]
    fn thumb_srcs_appends_cache_bust_query_when_bust_is_nonzero() {
        let book = book_with_cover(Some("/api/covers/abc"));
        let (src, srcset) = thumb_srcs(&book, "abc", "", 3);
        assert_eq!(src.as_deref(), Some("/api/thumbs/abc/md?v=3"));
        assert_eq!(
            srcset.as_deref(),
            Some(
                "/api/thumbs/abc/sm?v=3 160w, /api/thumbs/abc/md?v=3 320w, /api/thumbs/abc/lg?v=3 640w"
            ),
        );
    }
}
