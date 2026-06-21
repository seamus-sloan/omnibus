//! Markup sub-components for the landing page. Stateless: each takes the
//! current snapshot of derived data and an `on_change` (or per-action)
//! handler so [`super::LandingPage`] keeps ownership of the canonical
//! `prefs` signal and the data pipeline.

use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, SortKey, ViewFilters, ViewMode, ViewPrefs};

use crate::components::chip_editor::SuggestionItem;

use super::filtering::FacetCounts;
use super::filters::{EmptyFiltered, FilterSidebar};
use super::grid::BookGrid;
use super::sorting::{default_dir_for, toggle_dir};
use super::table::{BookTable, BookTableContext};
use super::toolbar::Toolbar;

/// Sticky page header: kicker + title + count + toolbar, with the
/// `data-testid="lib-header"` contract the E2E specs hook into. Errors
/// (page-level fetch + library-path) render here too.
#[component]
pub(super) fn LandingHeader(
    path_subtitle: String,
    book_count: usize,
    prefs: ViewPrefs,
    on_prefs_change: EventHandler<ViewPrefs>,
    path_missing: bool,
    page_error: Option<String>,
    lib_err: Option<String>,
) -> Element {
    rsx! {
        header { class: "lib-header", "data-testid": "lib-header",
            div { class: "lib-header-kicker",
                h1 { class: "label lib-header-kicker-title", "Your Library" }
                if !path_subtitle.is_empty() {
                    span { class: "mono lib-header-path", " · {path_subtitle}" }
                }
            }
            div { class: "lib-header-row",
                p { class: "lib-header-title",
                    em { "{book_count}" }
                    " "
                    if book_count == 1 { "book" } else { "books" }
                }
                Toolbar {
                    prefs: prefs,
                    on_change: move |next: ViewPrefs| on_prefs_change.call(next),
                }
            }
            if path_missing {
                p { class: "lib-header-hint",
                    "Configure your ebook library path in Settings."
                }
            }
            if let Some(msg) = page_error.as_ref() {
                p { class: "error", "⚠ {msg}" }
            }
            if let Some(msg) = lib_err.as_ref() {
                p { class: "error", "⚠ {msg}" }
            }
        }
    }
}

/// Per-render snapshot of the data the table/grid + load-more sentinel need.
#[derive(Clone, PartialEq, Props)]
pub(super) struct LandingContentProps {
    pub is_loading: bool,
    pub visible_books: Vec<EbookMetadata>,
    pub visible_is_empty: bool,
    pub books_empty: bool,
    pub lib_err: Option<String>,
    pub page_error: Option<String>,
    pub view_mode: ViewMode,
    pub prefs: ViewPrefs,
    pub facet_counts_view: FacetCounts,
    pub has_more: bool,
    pub is_loading_more: bool,
    pub server_url: String,
    pub is_admin: bool,
    pub author_suggestions: ReadSignal<Vec<SuggestionItem>>,
    pub tag_suggestions: ReadSignal<Vec<SuggestionItem>>,
    pub on_prefs_change: EventHandler<ViewPrefs>,
    pub on_load_more: EventHandler<()>,
    pub on_clear_filters: EventHandler<()>,
}

/// Sidebar + main column: facet sidebar, the grid/table switch, the
/// load-more sentinel, and the empty/filtered fallbacks. Stateless —
/// every mutation routes back through the parent's handlers.
#[component]
pub(super) fn LandingContent(props: LandingContentProps) -> Element {
    let LandingContentProps {
        is_loading,
        visible_books,
        visible_is_empty,
        books_empty,
        lib_err,
        page_error,
        view_mode,
        prefs,
        facet_counts_view,
        has_more,
        is_loading_more,
        server_url,
        is_admin,
        author_suggestions,
        tag_suggestions,
        on_prefs_change,
        on_load_more,
        on_clear_filters,
    } = props;

    let layout_class = if prefs.filters_open {
        "lib-layout"
    } else {
        "lib-layout lib-layout--collapsed"
    };
    let prefs_for_sidebar = prefs.clone();
    let on_filters_change = {
        let prefs = prefs.clone();
        move |filters: ViewFilters| {
            let mut next = prefs.clone();
            next.filters = filters;
            on_prefs_change.call(next);
        }
    };
    let on_sort = {
        let prefs = prefs.clone();
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
    };

    rsx! {
        div { class: "{layout_class}",
            FilterSidebar {
                facets: facet_counts_view,
                filters: prefs_for_sidebar.filters.clone(),
                on_change: on_filters_change,
            }

            div { class: "lib-main",
                if is_loading {
                    p { class: "library-empty", "Loading..." }
                } else if !visible_is_empty || lib_err.is_some() || page_error.is_some() {
                    match view_mode {
                        ViewMode::Table => rsx! {
                            BookTable {
                                books: visible_books.clone(),
                                prefs: prefs.clone(),
                                on_sort,
                                ctx: BookTableContext {
                                    server_url: server_url.clone(),
                                    is_admin,
                                    author_suggestions,
                                    tag_suggestions,
                                },
                            }
                        },
                        ViewMode::Grid => rsx! {
                            BookGrid {
                                books: visible_books.clone(),
                                server_url: server_url.clone(),
                            }
                        },
                    }
                    // Browse pagination sentinel — the button is the
                    // deterministic (mobile + Playwright) trigger; on web an
                    // IntersectionObserver auto-bumps it as it nears the
                    // viewport. Absent in search mode (no `next_cursor`).
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
    }
}
