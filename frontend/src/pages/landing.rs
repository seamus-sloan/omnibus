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
use omnibus_shared::{EbookMetadata, FacetCounts as ServerFacetCounts, ViewFilters, ViewPrefs};

use crate::components::chip_editor::SuggestionItem;
use crate::{use_search_query, use_server_url, view_prefs};

mod effects;
mod filtering;
mod filters;
mod grid;
mod sections;
mod sorting;
mod table;
mod toolbar;

#[cfg(feature = "web")]
use effects::spawn_load_more_observer;
use effects::{
    spawn_load_more_effect, spawn_page_fetch_effect, spawn_suggestion_pools_effect, FetchSignals,
    SuggestionPools,
};
use filtering::{apply_filters, facet_counts, FacetCounts};
use filters::FormatChips;
use sections::{LandingContent, LandingContentProps, LandingHeader};
use sorting::sort_books;

/// Keyset page size for the browse path (F5b open question #1). A grid renders
/// ~30–60 cards above the fold and the table more; 100 covers both without an
/// oversized first paint.
pub(super) const PAGE_SIZE: i64 = 100;

/// Landing page — primary library surface. See the module doc for the
/// browse-paginates / search-stays-client-side split.
#[component]
pub fn LandingPage() -> Element {
    let server_url = use_server_url();
    // Accumulated result rows: one growing browse list, or the capped search
    // result set. `next_cursor` is `Some` only while more browse pages remain.
    let books = use_signal(Vec::<EbookMetadata>::new);
    let next_cursor = use_signal(|| None::<String>);
    let server_facets = use_signal(|| None::<ServerFacetCounts>);
    let total = use_signal(|| None::<i64>);
    let lib_path = use_signal(|| None::<String>);
    let lib_error = use_signal(|| None::<String>);
    let loading = use_signal(|| true);
    let loading_more = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    let mut prefs = use_signal(ViewPrefs::default);
    // Bumped by the Load-more button and the web scroll observer; one effect
    // watches it and appends the next page.
    let mut want_more = use_signal(|| 0u32);
    // Monotonic fetch epoch, bumped on every page-1 refetch. An in-flight
    // page-1 fetch or load-more append captures the epoch and drops its result
    // if a newer fetch (sort/dir/filter/query change) superseded it mid-flight
    // — otherwise it would splice an old result stream onto the new list.
    let fetch_epoch = use_signal(|| 0u64);
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
    let pools = SuggestionPools {
        authors: use_signal(Vec::<SuggestionItem>::new),
        tags: use_signal(Vec::<SuggestionItem>::new),
    };
    spawn_suggestion_pools_effect(server_url.clone(), is_admin, pools);

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
    let sigs = FetchSignals {
        books,
        next_cursor,
        server_facets,
        total,
        lib_path,
        lib_error,
        loading,
        loading_more,
        error,
        fetch_epoch,
    };
    spawn_page_fetch_effect(server_url.clone(), fetch_key, sigs);
    spawn_load_more_effect(server_url.clone(), want_more, prefs, sigs);
    #[cfg(feature = "web")]
    spawn_load_more_observer(next_cursor);

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

    let on_prefs_change = {
        let mut save = save.clone();
        move |next: ViewPrefs| save(next)
    };
    let on_formats_change = {
        let mut save = save.clone();
        move |formats: Vec<String>| {
            let mut next = prefs.peek().clone();
            next.filters.formats = formats;
            save(next);
        }
    };
    let on_load_more = move |_| {
        want_more.with_mut(|n| *n += 1);
    };
    let on_clear_filters = {
        let mut save = save.clone();
        move |_| {
            let mut next = prefs.peek().clone();
            next.filters = ViewFilters::default();
            save(next);
        }
    };
    let books_empty = books().is_empty();

    rsx! {
        LandingHeader {
            path_subtitle,
            book_count,
            prefs: prefs(),
            on_prefs_change: on_prefs_change.clone(),
            path_missing: path_value.is_none(),
            page_error: page_error.clone(),
            lib_err: lib_err.clone(),
        }

        FormatChips {
            counts: facet_counts_view.formats.clone(),
            visible_count,
            book_count,
            selected: filters_for_chips.formats.clone(),
            on_change: on_formats_change,
        }

        LandingContent {
            ..LandingContentProps {
                is_loading,
                visible_books,
                visible_is_empty,
                books_empty,
                lib_err,
                page_error,
                view_mode,
                prefs: prefs(),
                facet_counts_view,
                has_more,
                is_loading_more,
                server_url: server_url_for_row.clone(),
                is_admin: is_admin(),
                author_suggestions: pools.authors.into(),
                tag_suggestions: pools.tags.into(),
                on_prefs_change: EventHandler::new(on_prefs_change),
                on_load_more: EventHandler::new(on_load_more),
                on_clear_filters: EventHandler::new(on_clear_filters),
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
