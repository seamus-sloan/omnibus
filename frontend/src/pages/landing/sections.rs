//! Markup sub-components for the landing page. Stateless: each takes the
//! current snapshot of derived data and an `on_change` (or per-action)
//! handler so [`super::LandingPage`] keeps ownership of the canonical
//! `prefs` signal and the data pipeline.

use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, Shelf, SortKey, ViewMode, ViewPrefs};

use super::filters::EmptyFiltered;
use super::grid::BookGrid;
use super::sorting::{default_dir_for, toggle_dir};
use super::table::{BookTable, BookTableContext};
use super::toolbar::Toolbar;
use crate::components::shelf_facets::pencil_glyph;
use crate::components::ShelfFacets;

/// Header banner fields sourced from the page's derived view state.
#[derive(Clone, PartialEq)]
pub(super) struct LandingHeaderView {
    pub path_subtitle: String,
    pub book_count: usize,
    /// "N hidden" receipt (browse only, viewer has a hidden-formats pref).
    pub hidden_count: Option<i64>,
    pub path_missing: bool,
    pub page_error: Option<String>,
    pub lib_err: Option<String>,
    /// The gallery pick this header titles: "All Books" or the shelf's name.
    pub section_title: String,
    /// Full detail for the gallery pick (`None` on All Books / while it
    /// loads) — drives the edit pencil and the facet row.
    pub selected_shelf: Option<Shelf>,
}

/// Sticky `data-testid="lib-header"` header; also renders page-level +
/// library-path errors. When the gallery pick is a shelf, the title grows an
/// edit pencil (owner/admin, non-system) and a facet row (kind / visibility /
/// rule chips) beneath it.
#[component]
pub(super) fn LandingHeader(
    view: LandingHeaderView,
    prefs: ViewPrefs,
    on_prefs_change: EventHandler<ViewPrefs>,
    on_edit_shelf: EventHandler<()>,
) -> Element {
    let LandingHeaderView {
        path_subtitle,
        book_count,
        hidden_count,
        path_missing,
        page_error,
        lib_err,
        section_title,
        selected_shelf,
    } = view;
    // Owner/admin gating, mirroring `shelf_detail`'s header: `None` viewer
    // until the boot effect resolves, so the pencil stays hidden on SSR +
    // first paint (hydration parity, rule 07); system shelves stay locked.
    let viewer = crate::use_current_user_summary()();
    let can_edit = selected_shelf.as_ref().is_some_and(|s| {
        !s.kind.is_system()
            && viewer
                .as_ref()
                .is_some_and(|u| u.id == s.owner_user_id || u.is_admin)
    });
    rsx! {
        header { class: "lib-header", "data-testid": "lib-header",
            div { class: "lib-header-kicker",
                h1 { class: "label lib-header-kicker-title", "Your Library" }
                if !path_subtitle.is_empty() {
                    span { class: "mono lib-header-path", " · {path_subtitle}" }
                }
            }
            div { class: "lib-header-row",
                LandingHeaderTitleRow {
                    section_title,
                    book_count,
                    hidden_count,
                    can_edit,
                    on_edit_shelf,
                }
                Toolbar {
                    prefs: prefs,
                    on_change: move |next: ViewPrefs| on_prefs_change.call(next),
                }
            }
            if let Some(shelf) = selected_shelf.as_ref() {
                ShelfFacets { shelf: shelf.clone() }
            }
            LandingHeaderMessages { path_missing, page_error, lib_err }
        }
    }
}

/// Section title + book count + hidden-formats receipt + the shelf edit
/// pencil, extracted from [`LandingHeader`] to keep it under the line cap.
#[component]
fn LandingHeaderTitleRow(
    section_title: String,
    book_count: usize,
    hidden_count: Option<i64>,
    can_edit: bool,
    on_edit_shelf: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "lib-header-title-wrap",
            p { class: "lib-header-title", "data-testid": "lib-section-title",
                em { "{section_title}" }
                span { class: "lib-header-count",
                    " · {book_count} "
                    if book_count == 1 { "book" } else { "books" }
                }
                // The receipt: hidden books must never look like data loss,
                // so the exclusion always shows its count.
                if let Some(n) = hidden_count.filter(|n| *n > 0) {
                    span {
                        class: "lib-header-hidden",
                        "data-testid": "lib-hidden-count",
                        " · {n} hidden"
                    }
                }
            }
            if can_edit {
                button {
                    r#type: "button",
                    class: "shelf-edit-btn",
                    "data-testid": "shelf-edit",
                    "aria-label": "Edit shelf",
                    onclick: move |_| on_edit_shelf.call(()),
                    {pencil_glyph()}
                }
            }
        }
    }
}

/// Path-missing hint + page-level + library-path error banners, extracted
/// from [`LandingHeader`] to keep it under the line cap.
#[component]
fn LandingHeaderMessages(
    path_missing: bool,
    page_error: Option<String>,
    lib_err: Option<String>,
) -> Element {
    rsx! {
        if path_missing {
            p { class: "lib-header-hint",
                "Configure your ebook library path in Settings."
            }
        }
        if let Some(msg) = page_error.as_ref() {
            // Tagged because a failed shelf-member fetch and a genuinely
            // empty shelf both render an empty grid — this banner is the
            // only thing that tells them apart.
            p { class: "error", "data-testid": "lib-page-error", "⚠ {msg}" }
        }
        if let Some(msg) = lib_err.as_ref() {
            p { class: "error", "⚠ {msg}" }
        }
    }
}

/// Result of the book fetch/filter pipeline: what `lib-main` should render
/// and whether the load-more sentinel should appear.
#[derive(Clone, PartialEq)]
pub(super) struct BooksView {
    pub is_loading: bool,
    pub visible_books: Vec<EbookMetadata>,
    pub visible_is_empty: bool,
    pub books_empty: bool,
    pub lib_err: Option<String>,
    pub page_error: Option<String>,
    pub has_more: bool,
    pub is_loading_more: bool,
}

/// Event handlers dispatched from the sidebar/grid/table/pagination row.
#[derive(Clone, PartialEq)]
pub(super) struct LandingContentHandlers {
    pub on_prefs_change: EventHandler<ViewPrefs>,
    pub on_load_more: EventHandler<()>,
    pub on_clear_filters: EventHandler<()>,
}

/// Per-render snapshot of the data the table/grid + load-more sentinel need.
#[derive(Clone, PartialEq, Props)]
pub(super) struct LandingContentProps {
    pub books: BooksView,
    pub prefs: ViewPrefs,
    pub ctx: BookTableContext,
    pub handlers: LandingContentHandlers,
    /// Remount key for the book area — a changed key mounts a fresh subtree,
    /// which is what replays the sweep-in CSS cascade on a gallery pick.
    pub sweep_key: String,
}

/// Sidebar + grid/table column with load-more sentinel; stateless, mutations route through parent handlers.
#[component]
pub(super) fn LandingContent(props: LandingContentProps) -> Element {
    let LandingContentProps {
        books,
        prefs,
        ctx,
        handlers,
        sweep_key,
    } = props;
    let LandingContentHandlers {
        on_prefs_change,
        on_load_more,
        on_clear_filters,
    } = handlers;
    let on_sort = build_sort_handler(prefs.clone(), on_prefs_change);

    rsx! {
        div { class: "lib-layout lib-layout--collapsed",
            div { key: "{sweep_key}", class: "lib-main lib-books",
                LandingBooksArea {
                    books,
                    prefs,
                    ctx,
                    on_sort: EventHandler::new(on_sort),
                    on_load_more,
                    on_clear_filters,
                }
            }
        }
    }
}

/// Builds the table/grid sort-column handler: clicking the already-active
/// column toggles direction, else adopts the new axis's natural direction.
fn build_sort_handler(
    prefs: ViewPrefs,
    on_prefs_change: EventHandler<ViewPrefs>,
) -> impl FnMut(SortKey) + 'static {
    move |key: SortKey| {
        let mut next = prefs.clone();
        next.sort_dir = if next.sort_key == key {
            toggle_dir(next.sort_dir)
        } else {
            default_dir_for(key)
        };
        next.sort_key = key;
        on_prefs_change.call(next);
    }
}

/// The loading / table-or-grid / empty / filtered-empty states for the main
/// book area, and the load-more pagination sentinel.
#[component]
fn LandingBooksArea(
    books: BooksView,
    prefs: ViewPrefs,
    ctx: BookTableContext,
    on_sort: EventHandler<SortKey>,
    on_load_more: EventHandler<()>,
    on_clear_filters: EventHandler<()>,
) -> Element {
    let BooksView {
        is_loading,
        visible_books,
        visible_is_empty,
        books_empty,
        lib_err,
        page_error,
        has_more,
        is_loading_more,
    } = books;
    let view_mode = prefs.view_mode;

    rsx! {
        if is_loading {
            p { class: "library-empty", "Loading..." }
        } else if !visible_is_empty || lib_err.is_some() || page_error.is_some() {
            match view_mode {
                ViewMode::Table => rsx! {
                    BookTable {
                        books: visible_books.clone(),
                        prefs: prefs.clone(),
                        on_sort,
                        ctx: ctx.clone(),
                    }
                },
                ViewMode::Grid => rsx! {
                    BookGrid {
                        books: visible_books.clone(),
                        server_url: ctx.server_url.clone(),
                    }
                },
            }
            // Browse pagination sentinel — the button is the deterministic
            // (mobile + Playwright) trigger; on web an IntersectionObserver
            // auto-bumps it as it nears the viewport. Absent in search mode
            // (no `next_cursor`).
            if has_more {
                div { class: "lib-load-more-row",
                    button {
                        class: "btn lib-load-more",
                        "data-testid": "lib-load-more",
                        disabled: is_loading_more,
                        onclick: move |_| on_load_more.call(()),
                        if is_loading_more { "Loading…" } else { "Load more" }
                    }
                }
            }
        } else if books_empty {
            p { class: "library-empty", "No ebooks found." }
        } else {
            EmptyFiltered {
                on_clear: move |_| on_clear_filters.call(()),
            }
        }
    }
}
