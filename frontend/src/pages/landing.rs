//! Landing page (`/`) — the primary library surface. See [`LandingPage`]
//! for the browse-paginates / search-stays-client-side split. View mode +
//! sort + filters persist per library path via [`crate::view_prefs`].

use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, ViewFilters, ViewPrefs};

use crate::components::chip_editor::SuggestionItem;
#[cfg(not(feature = "mobile"))]
use crate::components::{RailActive, ShelvesRail};
use crate::{use_search_query, use_server_url, view_prefs};

mod effects;
mod filtering;
mod sorting;

// Web-only presentation cluster (rail + toolbar + table/grid). The mobile
// build renders `mobile::MobileLanding` instead and never pulls these in.
#[cfg(not(feature = "mobile"))]
mod filters;
#[cfg(not(feature = "mobile"))]
mod grid;
#[cfg(not(feature = "mobile"))]
mod sections;
#[cfg(not(feature = "mobile"))]
mod table;
#[cfg(not(feature = "mobile"))]
mod toolbar;

#[cfg(feature = "mobile")]
mod mobile;

#[cfg(feature = "web")]
use effects::spawn_load_more_observer;
use effects::{
    spawn_load_more_effect, spawn_page_fetch_effect, spawn_suggestion_pools_effect, FetchSignals,
    SuggestionPools,
};
use filtering::apply_filters;
#[cfg(not(feature = "mobile"))]
use sections::{
    BooksView, LandingContent, LandingContentHandlers, LandingContentProps, LandingHeader,
    LandingHeaderView,
};
use sorting::sort_books;
#[cfg(not(feature = "mobile"))]
use table::BookTableContext;

/// Keyset page size for the browse path (F5b open question #1). A grid renders
/// ~30–60 cards above the fold and the table more; 100 covers both without an
/// oversized first paint.
pub(super) const PAGE_SIZE: i64 = 100;

/// Owned handles for every reactive signal the landing page wires up.
/// Returned by [`setup_landing_signals`] so [`LandingPage`] can stay a thin
/// composition of named stages.
#[derive(Copy, Clone)]
// Mobile reads a subset of these (it renders a compact grid, no admin table or
// suggestion pools); the rest feed the web rail/toolbar only.
#[cfg_attr(feature = "mobile", allow(dead_code))]
struct LandingSignals {
    books: Signal<Vec<EbookMetadata>>,
    next_cursor: Signal<Option<String>>,
    total: Signal<Option<i64>>,
    lib_path: Signal<Option<String>>,
    lib_error: Signal<Option<String>>,
    loading: Signal<bool>,
    loading_more: Signal<bool>,
    error: Signal<Option<String>>,
    prefs: Signal<ViewPrefs>,
    want_more: Signal<u32>,
    is_admin: Signal<bool>,
    pools: SuggestionPools,
}

/// Construct every signal the landing page owns and arm the effects that
/// fetch data into them. Must run from inside [`LandingPage`] — every call
/// here is a Dioxus hook, so the call order is stable across renders.
fn setup_landing_signals(server_url: &str, query: Signal<String>) -> LandingSignals {
    // Accumulated result rows: one growing browse list, or the capped search
    // result set. `next_cursor` is `Some` only while more browse pages remain.
    let books = use_signal(Vec::<EbookMetadata>::new);
    let next_cursor = use_signal(|| None::<String>);
    let total = use_signal(|| None::<i64>);
    let lib_path = use_signal(|| None::<String>);
    let lib_error = use_signal(|| None::<String>);
    let loading = use_signal(|| true);
    let loading_more = use_signal(|| false);
    let error = use_signal(|| None::<String>);
    let prefs = use_signal(ViewPrefs::default);
    // Bumped by the Load-more button and the web scroll observer; one effect
    // watches it and appends the next page.
    let want_more = use_signal(|| 0u32);
    // Monotonic fetch epoch, bumped on every page-1 refetch. An in-flight
    // page-1 fetch or load-more append captures the epoch and drops its result
    // if a newer fetch (sort/dir/filter/query change) superseded it mid-flight
    // — otherwise it would splice an old result stream onto the new list.
    let fetch_epoch = use_signal(|| 0u64);
    let is_admin = crate::use_is_admin();
    // Suggestion pools for the inline Authors chip editor and the
    // (future-reserved) Tags pool, each carrying the dropdown book-count.
    let pools = SuggestionPools {
        authors: use_signal(Vec::<SuggestionItem>::new),
        tags: use_signal(Vec::<SuggestionItem>::new),
    };
    let fetch_sigs = FetchSignals {
        books,
        next_cursor,
        total,
        lib_path,
        lib_error,
        loading,
        loading_more,
        error,
        fetch_epoch,
    };
    wire_landing_effects(
        server_url, query, prefs, want_more, is_admin, pools, fetch_sigs,
    );
    LandingSignals {
        books,
        next_cursor,
        total,
        lib_path,
        lib_error,
        loading,
        loading_more,
        error,
        prefs,
        want_more,
        is_admin,
        pools,
    }
}

/// Arm every reactive side-effect the landing page needs: admin-gated
/// suggestion-pool refetch, page-1 fetch on sort/filter/query change, the
/// load-more append + web scroll observer, and prefs hydration once the
/// library path resolves.
fn wire_landing_effects(
    server_url: &str,
    query: Signal<String>,
    mut prefs: Signal<ViewPrefs>,
    want_more: Signal<u32>,
    is_admin: Signal<bool>,
    pools: SuggestionPools,
    fetch_sigs: FetchSignals,
) {
    spawn_suggestion_pools_effect(server_url.to_string(), is_admin, pools);

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
    spawn_page_fetch_effect(server_url.to_string(), fetch_key, fetch_sigs);
    spawn_load_more_effect(server_url.to_string(), want_more, prefs, fetch_sigs);
    #[cfg(feature = "web")]
    spawn_load_more_observer(fetch_sigs.next_cursor);

    // Hydrate persisted prefs when the library path resolves. The `!=` guard
    // makes this idempotent: re-running it after a page-1 refetch (which re-sets
    // `lib_path`) is a no-op once prefs match, so it can't loop with the
    // fetch effect.
    let lib_path = fetch_sigs.lib_path;
    use_effect(move || {
        if let Some(path) = lib_path.read().clone() {
            let stored = view_prefs::load(&path);
            if stored != *prefs.peek() {
                prefs.set(stored);
            }
        }
    });
}

/// Per-render snapshot of the data the markup sub-components consume.
/// Computed by [`derive_view_state`] so the [`LandingPage`] body is just
/// composition.
#[cfg_attr(feature = "mobile", allow(dead_code))]
struct LandingViewState {
    is_loading: bool,
    page_error: Option<String>,
    lib_err: Option<String>,
    path_subtitle: String,
    path_missing: bool,
    book_count: usize,
    visible_books: Vec<EbookMetadata>,
    visible_is_empty: bool,
    books_empty: bool,
    has_more: bool,
    is_loading_more: bool,
}

/// Snapshot every signal the markup needs in one place. Reads are cheap, but
/// doing them inline in `rsx!` would multiply each `prefs()`/`books()` call
/// across the three child components.
fn derive_view_state(sigs: &LandingSignals, query: Signal<String>) -> LandingViewState {
    // Browse is already server-ordered + server-filtered; render `books`
    // verbatim. Search sorts + filters the capped result set client-side.
    let books_sig = sigs.books;
    let prefs_sig = sigs.prefs;
    let visible = use_memo(move || {
        let is_search = !query().trim().is_empty();
        if is_search {
            let bs = books_sig.read();
            let p = prefs_sig.read();
            sort_books(apply_filters(&bs, &p.filters), p.sort_key, p.sort_dir)
        } else {
            // Browse renders the server-ordered list verbatim; the memo stores
            // a `Vec` by value, so a clone is unavoidable on this branch.
            books_sig()
        }
    });

    let is_search = !query().trim().is_empty();
    let path_value = (sigs.lib_path)();
    // Header count: the full library total on browse; the (capped) result
    // count on search.
    let book_count = if is_search {
        sigs.books.read().len()
    } else {
        (sigs.total)()
            .map(|t| usize::try_from(t).unwrap_or(0))
            .unwrap_or_else(|| sigs.books.read().len())
    };
    let visible_books = visible();
    let visible_is_empty = visible_books.is_empty();
    let path_subtitle = path_value
        .as_ref()
        .map(|p| short_path(p))
        .unwrap_or_default();

    LandingViewState {
        is_loading: (sigs.loading)(),
        page_error: (sigs.error)(),
        lib_err: (sigs.lib_error)(),
        path_subtitle,
        path_missing: path_value.is_none(),
        book_count,
        visible_books,
        visible_is_empty,
        books_empty: sigs.books.read().is_empty(),
        has_more: (sigs.next_cursor)().is_some(),
        is_loading_more: (sigs.loading_more)(),
    }
}

/// `EventHandler` bundle dispatched into by the markup sub-components.
/// Built by [`build_handlers`] from the owned signals so the
/// [`LandingPage`] body stays a thin composition.
#[cfg_attr(feature = "mobile", allow(dead_code))]
struct LandingHandlers {
    on_prefs_change_header: EventHandler<ViewPrefs>,
    on_prefs_change_content: EventHandler<ViewPrefs>,
    on_load_more: EventHandler<()>,
    on_clear_filters: EventHandler<()>,
}

/// Build the UI-event handlers from the landing signals. `save` is `Copy`
/// because every capture (`prefs`, `lib_path` — both `Signal`) is `Copy`,
/// so each handler can take its own reference to the same persisted-prefs
/// update path without cloning closure state.
fn build_handlers(sigs: &LandingSignals) -> LandingHandlers {
    let mut prefs = sigs.prefs;
    let lib_path = sigs.lib_path;
    let mut want_more = sigs.want_more;
    let save = move |new_prefs: ViewPrefs| {
        if let Some(path) = lib_path.peek().as_ref() {
            view_prefs::save(path, &new_prefs);
        }
        prefs.set(new_prefs);
    };
    LandingHandlers {
        on_prefs_change_header: EventHandler::new({
            let mut save = save;
            move |next: ViewPrefs| save(next)
        }),
        on_prefs_change_content: EventHandler::new({
            let mut save = save;
            move |next: ViewPrefs| save(next)
        }),
        on_load_more: EventHandler::new(move |_: ()| {
            want_more.with_mut(|n| *n += 1);
        }),
        on_clear_filters: EventHandler::new({
            let mut save = save;
            move |_: ()| {
                let mut next = prefs.peek().clone();
                next.filters = ViewFilters::default();
                save(next);
            }
        }),
    }
}

/// Landing page — primary library surface.
///
/// Browse (no search query) is keyset-paginated server-side: the first page
/// carries the sidebar facets + the full-library count, and further pages
/// are appended from a "Load more" sentinel (auto-triggered on web by an
/// `IntersectionObserver`). Sort and filter are owned by the server —
/// changing either refetches page 1. Search (non-empty query) keeps the
/// legacy path: the capped result set is sorted/filtered client-side.
#[component]
pub fn LandingPage() -> Element {
    let server_url = use_server_url();
    // Search box lives in the top nav; the query is shared via context.
    let query = use_search_query().0;
    let sigs = setup_landing_signals(&server_url, query);
    let view = derive_view_state(&sigs, query);
    let handlers = build_handlers(&sigs);

    // Mobile renders a dedicated "Your shelf" surface; web keeps the
    // rail + toolbar layout. Both consume the shared data pipeline above —
    // only the presentation branches. (Mobile is a separate build, so this
    // cfg split doesn't affect web SSR/WASM hydration parity — rule 07.)
    #[cfg(feature = "mobile")]
    let body = rsx! {
        mobile::MobileLanding {
            book_count: view.book_count,
            books: view.visible_books,
            is_loading: view.is_loading,
            has_more: view.has_more,
            is_loading_more: view.is_loading_more,
            on_load_more: handlers.on_load_more,
            server_url,
        }
    };

    #[cfg(not(feature = "mobile"))]
    let body = {
        let prefs = sigs.prefs;
        let LandingHandlers {
            on_prefs_change_header,
            on_prefs_change_content,
            on_load_more,
            on_clear_filters,
        } = handlers;
        rsx! {
            div { class: "shelf-layout",
                ShelvesRail { active: RailActive::All }
                div { class: "shelf-main",
                    LandingHeader {
                        view: LandingHeaderView {
                            path_subtitle: view.path_subtitle,
                            book_count: view.book_count,
                            path_missing: view.path_missing,
                            page_error: view.page_error.clone(),
                            lib_err: view.lib_err.clone(),
                        },
                        prefs: prefs(),
                        on_prefs_change: on_prefs_change_header,
                    }

                    LandingContent {
                        ..LandingContentProps {
                            books: BooksView {
                                is_loading: view.is_loading,
                                visible_books: view.visible_books,
                                visible_is_empty: view.visible_is_empty,
                                books_empty: view.books_empty,
                                lib_err: view.lib_err,
                                page_error: view.page_error,
                                has_more: view.has_more,
                                is_loading_more: view.is_loading_more,
                            },
                            prefs: prefs(),
                            ctx: BookTableContext {
                                server_url,
                                is_admin: (sigs.is_admin)(),
                                author_suggestions: sigs.pools.authors.into(),
                                tag_suggestions: sigs.pools.tags.into(),
                            },
                            handlers: LandingContentHandlers {
                                on_prefs_change: on_prefs_change_content,
                                on_load_more,
                                on_clear_filters,
                            },
                        }
                    }
                }
            }
        }
    };

    body
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
