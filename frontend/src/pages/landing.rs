//! Landing page (`/`) — the primary library surface.
//!
//! Browse (no search query) is keyset-paginated server-side (F5b): the first
//! page carries the sidebar facets + the full-library count, and further pages
//! are appended from a "Load more" sentinel (auto-triggered on web by an
//! `IntersectionObserver`). Sort and filter are owned by the server — changing
//! either refetches page 1. Search (non-empty query) keeps the pre-F5b path:
//! the capped result set is sorted/filtered client-side (search keyset is a
//! separate effort). View mode + sort + filters persist per library path via
//! [`crate::view_prefs`].

use dioxus::prelude::*;
use omnibus_shared::{
    EbookMetadata, FacetCounts as ServerFacetCounts, SortKey, TagWeight, ViewFilters, ViewMode,
    ViewPrefs,
};

use crate::components::chip_editor::{collect_suggestions, SuggestionItem};
use crate::{data, use_search_query, use_server_url, view_prefs};

mod filtering;
mod filters;
mod grid;
mod sorting;
mod table;
mod toolbar;

use filtering::{apply_filters, facet_counts, FacetCounts};
use filters::{EmptyFiltered, FilterSidebar, FormatChips};
use grid::BookGrid;
use sorting::{default_dir_for, sort_books, toggle_dir};
use table::{BookTable, BookTableContext};
use toolbar::Toolbar;

/// Keyset page size for the browse path (F5b open question #1). A grid renders
/// ~30–60 cards above the fold and the table more; 100 covers both without an
/// oversized first paint.
const PAGE_SIZE: i64 = 100;

/// Landing page — primary library surface. See the module doc for the
/// browse-paginates / search-stays-client-side split.
#[component]
pub fn LandingPage() -> Element {
    let server_url = use_server_url();
    // Accumulated result rows: one growing browse list, or the capped search
    // result set. `next_cursor` is `Some` only while more browse pages remain.
    let mut books = use_signal(Vec::<EbookMetadata>::new);
    let mut next_cursor = use_signal(|| None::<String>);
    let mut server_facets = use_signal(|| None::<ServerFacetCounts>);
    let mut total = use_signal(|| None::<i64>);
    let mut lib_path = use_signal(|| None::<String>);
    let mut lib_error = use_signal(|| None::<String>);
    let mut loading = use_signal(|| true);
    let mut loading_more = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut prefs = use_signal(ViewPrefs::default);
    // Bumped by the Load-more button and the web scroll observer; one effect
    // watches it and appends the next page.
    let mut want_more = use_signal(|| 0u32);
    // Monotonic fetch epoch, bumped on every page-1 refetch. An in-flight
    // page-1 fetch or load-more append captures the epoch and drops its result
    // if a newer fetch (sort/dir/filter/query change) superseded it mid-flight
    // — otherwise it would splice an old result stream onto the new list.
    let mut fetch_epoch = use_signal(|| 0u64);
    // Search box lives in the top nav; the query is shared via context.
    let query = use_search_query().0;

    // F5.9-lite: admin-only inline-edit affordances on the power-user table.
    // Reads from the App-wide `CurrentUser` context — no per-mount round-trip.
    #[cfg_attr(not(feature = "web"), allow(unused_mut))]
    let mut is_admin = use_signal(|| false);
    #[cfg(feature = "web")]
    {
        let user_ctx = crate::use_current_user().0;
        use_effect(move || {
            is_admin.set(matches!(user_ctx(), Some(Some(ref u)) if u.is_admin));
        });
    }

    // Suggestion pools for the inline Authors chip editor and the
    // (future-reserved) Tags pool, each carrying the dropdown book-count.
    let mut author_suggestions: Signal<Vec<SuggestionItem>> = use_signal(Vec::new);
    let mut tag_suggestions: Signal<Vec<SuggestionItem>> = use_signal(Vec::new);
    {
        let url = server_url.clone();
        use_effect(move || {
            if !is_admin() {
                return;
            }
            let url = url.clone();
            spawn(async move {
                if let Ok(authors) = data::list_authors(&url).await {
                    let items: Vec<SuggestionItem> = authors
                        .into_iter()
                        .map(|a| SuggestionItem::new(a.name, a.book_count))
                        .collect();
                    author_suggestions.set(collect_suggestions(items));
                }
                if let Ok(tags) = data::get_tag_cloud(&url).await {
                    let items: Vec<SuggestionItem> = tags
                        .into_iter()
                        .map(|t: TagWeight| SuggestionItem::new(t.name, t.count))
                        .collect();
                    tag_suggestions.set(collect_suggestions(items));
                }
            });
        });
    }

    // Refetch page 1 whenever the search query or a *data-affecting* pref
    // (sort axis/dir or filters) changes. The `use_memo` keys the effect on
    // exactly those fields, so view-mode / sidebar-open toggles don't refetch.
    let fetch_key = use_memo(move || {
        let p = prefs();
        (
            query().trim().to_string(),
            p.sort_key,
            p.sort_dir,
            p.filters.clone(),
        )
    });
    {
        let url = server_url.clone();
        use_effect(move || {
            let (q, sort_key, sort_dir, filters) = fetch_key();
            let epoch = {
                fetch_epoch.with_mut(|e| *e += 1);
                *fetch_epoch.peek()
            };
            let url = url.clone();
            spawn(async move {
                loading.set(true);
                error.set(None);
                next_cursor.set(None);
                if q.is_empty() {
                    // Browse: server keyset page 1 (server-side sort + filter,
                    // facets + total ride along on the first page).
                    let result =
                        data::get_ebooks_page(&url, sort_key, sort_dir, filters, None, PAGE_SIZE)
                            .await;
                    if *fetch_epoch.peek() != epoch {
                        return; // a newer fetch superseded us — drop this result
                    }
                    match result {
                        Ok(page) => {
                            lib_path.set(page.path);
                            lib_error.set(None);
                            next_cursor.set(page.next_cursor);
                            total.set(page.total);
                            server_facets.set(page.facets);
                            books.set(page.books);
                        }
                        Err(e) => {
                            error.set(Some(e.to_string()));
                            // Clear the derived signals so the error state doesn't
                            // render a stale header count or facet sidebar.
                            books.set(Vec::new());
                            total.set(None);
                            server_facets.set(None);
                            lib_error.set(None);
                        }
                    }
                } else {
                    // Search: capped full result set, sorted/filtered client-side.
                    let result = data::search_ebooks(&url, &q).await;
                    if *fetch_epoch.peek() != epoch {
                        return;
                    }
                    match result {
                        Ok(lib) => {
                            lib_path.set(lib.path);
                            lib_error.set(lib.error);
                            total.set(lib.total);
                            server_facets.set(None); // client computes search facets
                            books.set(lib.books);
                        }
                        Err(e) => {
                            error.set(Some(e.to_string()));
                            books.set(Vec::new());
                            total.set(None);
                            server_facets.set(None);
                            lib_error.set(None);
                        }
                    }
                }
                loading.set(false);
            });
        });
    }

    // Append the next browse page when `want_more` is bumped (button click or
    // the web scroll observer). Guards against concurrent/empty loads.
    {
        let url = server_url.clone();
        use_effect(move || {
            let trigger = want_more();
            if trigger == 0 || *loading_more.peek() || next_cursor.peek().is_none() {
                return;
            }
            let p = prefs.peek().clone();
            let cursor = next_cursor.peek().clone();
            let epoch = *fetch_epoch.peek();
            let url = url.clone();
            loading_more.set(true);
            spawn(async move {
                let result = data::get_ebooks_page(
                    &url, p.sort_key, p.sort_dir, p.filters, cursor, PAGE_SIZE,
                )
                .await;
                // Drop the append if a page-1 refetch (sort/filter/query change)
                // superseded us mid-flight — otherwise we'd splice an old result
                // stream onto the new list and overwrite its cursor.
                if *fetch_epoch.peek() != epoch {
                    loading_more.set(false);
                    return;
                }
                match result {
                    Ok(page) => {
                        books.with_mut(|b| b.extend(page.books));
                        next_cursor.set(page.next_cursor);
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
                loading_more.set(false);
            });
        });
    }

    // Web: auto-load when the Load-more sentinel nears the viewport. The
    // `IntersectionObserver` simply `.click()`s the button, whose Dioxus
    // `onclick` bumps `want_more` — one-way `eval` only (the established
    // interop pattern; no markup divergence, since the button renders on every
    // target and only the observer is gated). The effect reads `next_cursor`
    // so it re-arms after each page append, disconnecting the prior observer
    // first so they can't accumulate.
    #[cfg(feature = "web")]
    {
        use_effect(move || {
            let _rearm_on = next_cursor.read().is_some();
            let _ = dioxus::document::eval(
                r#"
                if (window.__omnibusLoadMoreObs) {
                    window.__omnibusLoadMoreObs.disconnect();
                    window.__omnibusLoadMoreObs = null;
                }
                const el = document.querySelector('[data-testid="lib-load-more"]');
                if (el) {
                    const obs = new IntersectionObserver((entries) => {
                        if (entries.some((e) => e.isIntersecting)) { el.click(); }
                    }, { rootMargin: "400px" });
                    obs.observe(el);
                    window.__omnibusLoadMoreObs = obs;
                }
                "#,
            );
        });
    }

    // Hydrate persisted prefs when the library path resolves. The `!=` guard
    // makes this idempotent: re-running it after a page-1 refetch (which re-sets
    // `lib_path`) is a no-op once prefs match, so it can't loop with the
    // fetch effect.
    use_effect(move || {
        if let Some(path) = lib_path.read().clone() {
            let stored = view_prefs::load(&path);
            if stored != *prefs.peek() {
                prefs.set(stored);
            }
        }
    });

    // Browse is already server-ordered + server-filtered; render `books`
    // verbatim. Search sorts + filters the capped result set client-side.
    let visible = use_memo(move || {
        let is_search = !query().trim().is_empty();
        let bs = books();
        if is_search {
            let p = prefs();
            sort_books(apply_filters(&bs, &p.filters), p.sort_key, p.sort_dir)
        } else {
            bs
        }
    });
    // Facets come from the server on browse, client-tally on search.
    let facets = use_memo(move || match server_facets() {
        Some(s) => FacetCounts::from_shared(s),
        None => facet_counts(&books()),
    });

    let is_loading = loading();
    let page_error = error();
    let path_value = lib_path();
    let lib_err = lib_error();
    let is_search = !query().trim().is_empty();
    // Header count: the full library total on browse; the (capped) result
    // count on search.
    let book_count = if is_search {
        books().len()
    } else {
        total()
            .map(|t| usize::try_from(t).unwrap_or(0))
            .unwrap_or_else(|| books().len())
    };
    let view_mode = prefs().view_mode;
    let visible_books = visible();
    let visible_is_empty = visible_books.is_empty();
    let facet_counts_view = facets();
    let has_more = next_cursor().is_some();
    let is_loading_more = loading_more();

    let server_url_for_row = server_url.clone();
    let path_for_save = path_value.clone();
    let save = {
        let path = path_for_save.clone();
        move |new_prefs: ViewPrefs| {
            if let Some(path) = path.as_ref() {
                view_prefs::save(path, &new_prefs);
            }
            prefs.set(new_prefs);
        }
    };

    let path_subtitle = path_value
        .as_ref()
        .map(|p| short_path(p))
        .unwrap_or_default();
    let visible_count = visible_books.len();
    let filters_for_chips = prefs().filters.clone();

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
                    prefs: prefs(),
                    on_change: save.clone(),
                }
            }
            if path_value.is_none() {
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

        FormatChips {
            counts: facet_counts_view.formats.clone(),
            visible_count: visible_count,
            book_count: book_count,
            selected: filters_for_chips.formats.clone(),
            on_change: {
                let mut save = save.clone();
                move |formats: Vec<String>| {
                    let mut next = prefs.peek().clone();
                    next.filters.formats = formats;
                    save(next);
                }
            },
        }

        div { class: if prefs().filters_open { "lib-layout" } else { "lib-layout lib-layout--collapsed" },
            FilterSidebar {
                facets: facet_counts_view,
                filters: prefs().filters.clone(),
                on_change: {
                    let mut save = save.clone();
                    move |filters: ViewFilters| {
                        let mut next = prefs.peek().clone();
                        next.filters = filters;
                        save(next);
                    }
                },
            }

            div { class: "lib-main",
                if is_loading {
                    p { class: "library-empty", "Loading..." }
                } else if !visible_is_empty || lib_err.is_some() || page_error.is_some() {
                    match view_mode {
                        ViewMode::Table => rsx! {
                            BookTable {
                                books: visible_books.clone(),
                                prefs: prefs(),
                                on_sort: {
                                    let mut save = save.clone();
                                    move |key: SortKey| {
                                        let mut next = prefs.peek().clone();
                                        next.sort_dir = if next.sort_key == key {
                                            toggle_dir(next.sort_dir)
                                        } else {
                                            default_dir_for(key)
                                        };
                                        next.sort_key = key;
                                        save(next);
                                    }
                                },
                                ctx: BookTableContext {
                                    server_url: server_url_for_row.clone(),
                                    is_admin: is_admin(),
                                    author_suggestions: author_suggestions.into(),
                                    tag_suggestions: tag_suggestions.into(),
                                },
                            }
                        },
                        ViewMode::Grid => rsx! {
                            BookGrid {
                                books: visible_books.clone(),
                                server_url: server_url_for_row.clone(),
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
                                onclick: move |_| {
                                    want_more.with_mut(|n| *n += 1);
                                },
                                if is_loading_more { "Loading…" } else { "Load more" }
                            }
                        }
                    }
                } else if books().is_empty() {
                    p { class: "library-empty", "No ebooks found." }
                } else {
                    EmptyFiltered {
                        on_clear: {
                            let mut save = save.clone();
                            move |_| {
                                let mut next = prefs.peek().clone();
                                next.filters = ViewFilters::default();
                                save(next);
                            }
                        },
                    }
                }
            }
        }
    }
}

/// Short, human-friendly tail of an absolute library path. We show only the
/// last segment to keep the header line tidy — full path lives in Settings.
fn short_path(path: &str) -> String {
    path.rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or(path)
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_path_returns_last_segment() {
        assert_eq!(short_path("/Users/ek/books"), "books");
        assert_eq!(short_path("/Users/ek/books/"), "books");
        assert_eq!(short_path("relative"), "relative");
    }
}
