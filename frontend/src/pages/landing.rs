//! Landing page (`/`) — the primary library surface. See [`LandingPage`]
//! for the browse-paginates / search-stays-client-side split. View mode +
//! sort + filters persist per library path via [`crate::view_prefs`].

use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, ResumePoint, Shelf, ShelfSummary, ViewFilters, ViewPrefs};

use crate::components::chip_editor::SuggestionItem;
#[cfg(not(feature = "mobile"))]
use crate::components::EditShelfModal;
use crate::shelf_selection::{self, ShelfSelection};
use crate::{use_search_query, use_server_url, view_prefs};

mod effects;
mod filtering;
mod resume_meta;
mod sorting;

// Web-only presentation cluster (hero + gallery + toolbar + table/grid). The
// mobile build renders `mobile::MobileLanding` instead and never pulls these in.
#[cfg(not(feature = "mobile"))]
mod filters;
#[cfg(not(feature = "mobile"))]
mod grid;
#[cfg(not(feature = "mobile"))]
mod hero;
#[cfg(not(feature = "mobile"))]
mod sections;
#[cfg(not(feature = "mobile"))]
mod shelf_gallery;
#[cfg(not(feature = "mobile"))]
mod table;
#[cfg(not(feature = "mobile"))]
mod toolbar;

#[cfg(feature = "mobile")]
mod mobile;
#[cfg(feature = "mobile")]
mod mobile_filter_sheet;
#[cfg(feature = "mobile")]
mod pull_refresh;

// The mobile shelf-detail grid reuses the landing grid's cover cell so the
// two surfaces stay visually identical.
#[cfg(feature = "mobile")]
pub(crate) use mobile::cover_cell as mobile_cover_cell;

#[cfg(feature = "web")]
use effects::spawn_load_more_observer;
use effects::{
    spawn_hero_effect, spawn_load_more_effect, spawn_page_fetch_effect,
    spawn_selected_shelf_effect, spawn_shelf_books_effect, spawn_shelves_list_effect,
    spawn_suggestion_pools_effect, FetchSignals, ShelfFetchSignals, SuggestionPools,
};
use filtering::apply_filters;
#[cfg(not(feature = "mobile"))]
use hero::ContinueHero;
#[cfg(not(feature = "mobile"))]
use sections::{
    BooksView, LandingContent, LandingContentHandlers, LandingContentProps, LandingHeader,
    LandingHeaderView,
};
#[cfg(not(feature = "mobile"))]
use shelf_gallery::ShelfGallery;
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
    is_admin: ReadSignal<bool>,
    pools: SuggestionPools,
    /// Which lens the book list shows: All Books or one shelf (gallery pick).
    selection: Signal<ShelfSelection>,
    /// Gallery feed. Starts empty so the first WASM paint matches SSR.
    shelves: Signal<Vec<ShelfSummary>>,
    /// Bumped after a shelf create so the gallery refetches its list.
    shelves_tick: Signal<u32>,
    /// Selected shelf's member list (`None` = All Books / not yet loaded).
    shelf_books: Signal<Option<Vec<EbookMetadata>>>,
    shelf_loading: Signal<bool>,
    shelf_error: Signal<Option<String>>,
    /// Full detail for the selected shelf (`None` on All Books / while it
    /// loads) — feeds the header facet row and the edit-shelf modal.
    selected_shelf: Signal<Option<Shelf>>,
    /// True while the edit-shelf modal is open.
    edit_shelf: Signal<bool>,
    /// Continue-reading hero feed. Starts empty (hero hidden) for SSR parity.
    hero_points: Signal<Vec<ResumePoint>>,
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
    // Suggestion pools for the inline Authors and Tags chip editors, each
    // carrying the dropdown book-count.
    let pools = SuggestionPools {
        authors: use_signal(Vec::<SuggestionItem>::new),
        tags: use_signal(Vec::<SuggestionItem>::new),
    };
    // Shelf-gallery lens. `selection` seeds to All on every target (SSR
    // parity, rule 07); the persisted choice reconciles post-mount in
    // `wire_landing_effects`.
    let selection = use_signal(ShelfSelection::default);
    let shelves = use_signal(Vec::<ShelfSummary>::new);
    let shelves_loaded = use_signal(|| false);
    let shelves_tick = use_signal(|| 0u32);
    let shelf_books = use_signal(|| None::<Vec<EbookMetadata>>);
    let shelf_loading = use_signal(|| false);
    let shelf_error = use_signal(|| None::<String>);
    let shelf_epoch = use_signal(|| 0u64);
    let selected_shelf = use_signal(|| None::<Shelf>);
    let edit_shelf = use_signal(|| false);
    let hero_points = use_signal(Vec::<ResumePoint>::new);
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
        generation: crate::use_cache_generation(),
    };
    let shelf_sigs = ShelfFetchSignals {
        shelf_books,
        shelf_loading,
        shelf_error,
        shelf_epoch,
    };
    let shelf_wiring = ShelfWiring {
        selection,
        shelves,
        shelves_loaded,
        shelves_tick,
        shelf_sigs,
        selected_shelf,
        hero_points,
    };
    wire_landing_effects(
        server_url,
        query,
        prefs,
        want_more,
        is_admin,
        pools,
        fetch_sigs,
        shelf_wiring,
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
        selection,
        shelves,
        shelves_tick,
        shelf_books,
        shelf_loading,
        shelf_error,
        selected_shelf,
        edit_shelf,
        hero_points,
    }
}

/// Signal bundle for the shelf-gallery + hero effects armed by
/// [`wire_landing_effects`], so its signature stays readable.
#[derive(Copy, Clone)]
struct ShelfWiring {
    selection: Signal<ShelfSelection>,
    shelves: Signal<Vec<ShelfSummary>>,
    shelves_loaded: Signal<bool>,
    shelves_tick: Signal<u32>,
    shelf_sigs: ShelfFetchSignals,
    selected_shelf: Signal<Option<Shelf>>,
    hero_points: Signal<Vec<ResumePoint>>,
}

/// Arm every reactive side-effect the landing page needs: admin-gated
/// suggestion-pool refetch, page-1 fetch on sort/filter/query change, the
/// load-more append + web scroll observer, and prefs hydration once the
/// library path resolves.
#[allow(clippy::too_many_arguments)] // one bundle per pipeline; splitting further hides the wiring
fn wire_landing_effects(
    server_url: &str,
    query: Signal<String>,
    mut prefs: Signal<ViewPrefs>,
    want_more: Signal<u32>,
    is_admin: ReadSignal<bool>,
    pools: SuggestionPools,
    fetch_sigs: FetchSignals,
    shelf_wiring: ShelfWiring,
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

    let ShelfWiring {
        mut selection,
        shelves,
        shelves_loaded,
        shelves_tick,
        shelf_sigs,
        selected_shelf,
        hero_points,
    } = shelf_wiring;
    spawn_shelves_list_effect(
        server_url.to_string(),
        shelves_tick,
        shelves,
        shelves_loaded,
        selection,
    );
    spawn_hero_effect(server_url.to_string(), hero_points);

    // Full detail for the gallery pick; re-runs after an edit-shelf save
    // because the save bumps `shelves_tick`.
    let selected_key = use_memo(move || (selection(), shelves_tick()));
    spawn_selected_shelf_effect(server_url.to_string(), selected_key, selected_shelf);

    // Refetch the selected shelf's members when the gallery pick or the sort
    // axis changes, or after an edit-shelf save (`shelves_tick` bump — a rules
    // edit changes membership). Mirrors `fetch_key` deliberately: view-mode
    // toggles and filters stay client-side and must not refetch.
    let shelf_key = use_memo(move || {
        let p = prefs();
        (selection(), p.sort_key, p.sort_dir, shelves_tick())
    });
    spawn_shelf_books_effect(server_url.to_string(), shelf_key, shelf_sigs);

    // Reconcile the persisted gallery pick once after mount (no reactive
    // reads, so this runs exactly once). SSR's client_store is inert, so SSR
    // and the first WASM paint both render All Books — rule 07. A stored pick
    // whose shelf no longer exists snaps back to All: validated against the
    // shelves list here when it already arrived, and inside
    // `spawn_shelves_list_effect`'s completion when it hasn't — both are
    // plain sequential checks, never a reactive effect that could glitch on
    // partially-applied writes.
    use_effect(move || {
        let stored = shelf_selection::load();
        if stored == *selection.peek() {
            return;
        }
        if let ShelfSelection::Shelf(id) = stored {
            if *shelves_loaded.peek() && !shelves.peek().iter().any(|s| s.id == id) {
                shelf_selection::save(ShelfSelection::All);
                return;
            }
        }
        selection.set(stored);
    });
}

/// Which list feeds the grid/table. Search always wins (the palette overlays
/// everything); a gallery pick overlays browse; browse is the default.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
enum VisibleSource {
    Search,
    Shelf,
    Browse,
}

/// Resolve the precedence search > shelf > browse for this render.
fn visible_source(is_search: bool, selection: ShelfSelection) -> VisibleSource {
    if is_search {
        VisibleSource::Search
    } else if matches!(selection, ShelfSelection::Shelf(_)) {
        VisibleSource::Shelf
    } else {
        VisibleSource::Browse
    }
}

/// Section-header title for the current gallery pick. Falls back to a neutral
/// "Shelf" while the shelves list is still loading after a reload directly
/// into a persisted selection.
fn section_title(selection: ShelfSelection, shelves: &[ShelfSummary]) -> String {
    match selection {
        ShelfSelection::All => "All Books".to_string(),
        ShelfSelection::Shelf(id) => shelves
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.name.clone())
            .unwrap_or_else(|| "Shelf".to_string()),
    }
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
    section_title: String,
    /// Remount key for the book area: changing it replays the sweep-in
    /// cascade (a fresh subtree restarts its CSS animations).
    sweep_key: String,
    /// True while the palette query is non-empty — hero + gallery hide, and
    /// the search result set renders exactly as before the redesign.
    is_search: bool,
}

/// Snapshot every signal the markup needs in one place. Reads are cheap, but
/// doing them inline in `rsx!` would multiply each `prefs()`/`books()` call
/// across the three child components.
fn derive_view_state(sigs: &LandingSignals, query: Signal<String>) -> LandingViewState {
    // Browse is already server-ordered + server-filtered; render `books`
    // verbatim. Search sorts + filters the capped result set client-side. A
    // shelf pick renders its (server-sorted) member list, client-filtered
    // with the same helper the search path uses.
    let books_sig = sigs.books;
    let prefs_sig = sigs.prefs;
    let selection_sig = sigs.selection;
    let shelf_books_sig = sigs.shelf_books;
    let visible = use_memo(move || {
        let is_search = !query().trim().is_empty();
        match visible_source(is_search, selection_sig()) {
            VisibleSource::Search => {
                let bs = books_sig.read();
                let p = prefs_sig.read();
                sort_books(apply_filters(&bs, &p.filters), p.sort_key, p.sort_dir)
            }
            VisibleSource::Shelf => {
                let members = shelf_books_sig.read().clone().unwrap_or_default();
                let p = prefs_sig.read();
                apply_filters(&members, &p.filters)
            }
            // Browse renders the server-ordered list verbatim; the memo stores
            // a `Vec` by value, so a clone is unavoidable on this branch.
            VisibleSource::Browse => books_sig(),
        }
    });

    let is_search = !query().trim().is_empty();
    let selection = (sigs.selection)();
    let source = visible_source(is_search, selection);
    let shelves = sigs.shelves.read();
    let path_value = (sigs.lib_path)();
    // Header count: the full library total on browse; the shelf's member
    // count on a gallery pick; the (capped) result count on search.
    let book_count = match source {
        VisibleSource::Search => sigs.books.read().len(),
        VisibleSource::Shelf => {
            let member_len = sigs.shelf_books.read().as_ref().map(Vec::len).unwrap_or(0);
            match selection {
                ShelfSelection::Shelf(id) => shelves
                    .iter()
                    .find(|s| s.id == id)
                    .map(|s| usize::try_from(s.book_count).unwrap_or(member_len))
                    .unwrap_or(member_len),
                ShelfSelection::All => member_len,
            }
        }
        VisibleSource::Browse => (sigs.total)()
            .map(|t| usize::try_from(t).unwrap_or(0))
            .unwrap_or_else(|| sigs.books.read().len()),
    };
    let visible_books = visible();
    let visible_is_empty = visible_books.is_empty();
    let path_subtitle = path_value
        .as_ref()
        .map(|p| short_path(p))
        .unwrap_or_default();
    let is_loading = match source {
        VisibleSource::Shelf => (sigs.shelf_loading)(),
        _ => (sigs.loading)(),
    };
    let page_error = (sigs.error)().or_else(|| match source {
        VisibleSource::Shelf => (sigs.shelf_error)(),
        _ => None,
    });

    LandingViewState {
        is_loading,
        page_error,
        lib_err: (sigs.lib_error)(),
        path_subtitle,
        // A shelf pick isn't the surface for the library-path hint.
        path_missing: path_value.is_none() && source != VisibleSource::Shelf,
        book_count,
        visible_books,
        visible_is_empty,
        books_empty: match source {
            VisibleSource::Shelf => visible_is_empty,
            _ => sigs.books.read().is_empty(),
        },
        // Keyset pagination exists only on the browse path; shelf pages are
        // capped whole lists and search is a capped set.
        has_more: source == VisibleSource::Browse && (sigs.next_cursor)().is_some(),
        is_loading_more: (sigs.loading_more)(),
        section_title: section_title(selection, &shelves),
        sweep_key: format!("{selection:?}·{is_search}"),
        is_search,
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
    /// Gallery pick: move the glow, persist the choice, swap the book list.
    on_select_shelf: EventHandler<ShelfSelection>,
    /// After a create in the gallery's modal: refetch the shelves list.
    on_shelf_created: EventHandler<()>,
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
        on_select_shelf: EventHandler::new({
            let mut selection = sigs.selection;
            move |sel: ShelfSelection| {
                shelf_selection::save(sel);
                selection.set(sel);
            }
        }),
        on_shelf_created: EventHandler::new({
            let mut tick = sigs.shelves_tick;
            move |_: ()| tick.with_mut(|n| *n += 1)
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

    // Restore scroll on back-navigation once page 1 has loaded. The paginated
    // list is short on a fresh remount, so the restore retries across frames
    // while the load-more observer streams pages in toward the saved offset
    // (see `crate::scroll_restore`).
    let loading = sigs.loading;
    let content_ready = use_memo(move || !loading());
    crate::scroll_restore::use_scroll_restore(content_ready);

    // Mobile renders a dedicated "All Books" surface; web keeps the
    // rail + toolbar layout. Both consume the shared data pipeline above —
    // only the presentation branches. (Mobile is a separate build, so this
    // cfg split doesn't affect web SSR/WASM hydration parity — rule 07.)
    #[cfg(feature = "mobile")]
    let body = mobile_landing_body(&sigs, view, handlers, server_url);

    #[cfg(not(feature = "mobile"))]
    let body = web_landing_body(&sigs, view, handlers, server_url);

    body
}

/// Mobile presentation branch of [`LandingPage`]: a single compact grid plus
/// the continue card and sort & filter sheet — no admin table or suggestion
/// pools.
#[cfg(feature = "mobile")]
fn mobile_landing_body(
    sigs: &LandingSignals,
    view: LandingViewState,
    handlers: LandingHandlers,
    server_url: String,
) -> Element {
    // Hooks stay unconditional within the mobile build: this helper is on
    // LandingPage's only render path there.
    pull_refresh::use_pull_to_refresh(server_url.clone(), sigs.prefs);
    rsx! {
        mobile::MobileLanding {
            book_count: view.book_count,
            books: view.visible_books,
            is_loading: view.is_loading,
            has_more: view.has_more,
            is_loading_more: view.is_loading_more,
            on_load_more: handlers.on_load_more,
            prefs: (sigs.prefs)(),
            on_prefs_change: handlers.on_prefs_change_header,
            server_url,
        }
    }
}

/// Web presentation branch of [`LandingPage`]: continue-reading hero + shelf
/// gallery + section header + grid/table content, wired to the admin table
/// context and suggestion pools. The gallery filters in place — the old
/// shelves rail (still used on `/shelves/:id`) no longer mounts here.
#[cfg(not(feature = "mobile"))]
fn web_landing_body(
    sigs: &LandingSignals,
    view: LandingViewState,
    handlers: LandingHandlers,
    server_url: String,
) -> Element {
    let prefs = sigs.prefs;
    let LandingHandlers {
        on_prefs_change_header,
        on_prefs_change_content,
        on_load_more,
        on_clear_filters,
        on_select_shelf,
        on_shelf_created,
    } = handlers;
    let hero_points = (sigs.hero_points)();
    let selected_shelf = (sigs.selected_shelf)();
    let mut edit_shelf = sigs.edit_shelf;
    let mut shelves_tick = sigs.shelves_tick;
    // All Books mosaic: first four cover-bearing books of the (always-warm)
    // browse page. Before it lands, the tile falls back to its accent plate.
    let all_cover_uuids: Vec<String> = sigs
        .books
        .read()
        .iter()
        .filter(|b| b.cover_url.is_some())
        .filter_map(|b| b.unique_identifier.clone())
        .take(4)
        .collect();
    rsx! {
        div { class: "landing-col",
            if !view.is_search && !hero_points.is_empty() {
                ContinueHero {
                    points: hero_points,
                    server_url: server_url.clone(),
                }
            }
            if !view.is_search {
                ShelfGallery {
                    shelves: (sigs.shelves)(),
                    selection: (sigs.selection)(),
                    all_count: (sigs.total)(),
                    all_cover_uuids,
                    server_url: server_url.clone(),
                    on_select: on_select_shelf,
                    on_created: on_shelf_created,
                }
            }
            LandingHeader {
                view: LandingHeaderView {
                    path_subtitle: view.path_subtitle,
                    book_count: view.book_count,
                    path_missing: view.path_missing,
                    page_error: view.page_error.clone(),
                    lib_err: view.lib_err.clone(),
                    section_title: view.section_title,
                    selected_shelf: selected_shelf.clone(),
                },
                prefs: prefs(),
                on_prefs_change: on_prefs_change_header,
                on_edit_shelf: move |_| edit_shelf.set(true),
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
                    sweep_key: view.sweep_key,
                }
            }

            if edit_shelf() {
                if let Some(shelf) = selected_shelf {
                    EditShelfModal {
                        shelf,
                        on_close: move |_| edit_shelf.set(false),
                        on_saved: move |_| {
                            edit_shelf.set(false);
                            // Refetches the gallery list, the section title,
                            // and (via `selected_key`) the full shelf.
                            shelves_tick.with_mut(|n| *n += 1);
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

    #[test]
    fn visible_source_prefers_search_then_shelf_then_browse() {
        assert_eq!(
            visible_source(true, ShelfSelection::Shelf(1)),
            VisibleSource::Search
        );
        assert_eq!(
            visible_source(false, ShelfSelection::Shelf(1)),
            VisibleSource::Shelf
        );
        assert_eq!(
            visible_source(false, ShelfSelection::All),
            VisibleSource::Browse
        );
    }

    #[test]
    fn section_title_names_the_pick_and_falls_back_while_shelves_load() {
        let shelf = ShelfSummary {
            id: 3,
            owner_user_id: 1,
            owner_username: "elena".into(),
            kind: omnibus_shared::ShelfKind::Manual,
            name: "Space Operas".into(),
            visibility: omnibus_shared::Visibility::Private,
            accent: None,
            book_count: 2,
            cover_uuids: Vec::new(),
        };
        assert_eq!(section_title(ShelfSelection::All, &[]), "All Books");
        assert_eq!(
            section_title(ShelfSelection::Shelf(3), std::slice::from_ref(&shelf)),
            "Space Operas"
        );
        // Reloading straight into a persisted pick renders before the list
        // arrives — the header must not panic or claim All Books.
        assert_eq!(section_title(ShelfSelection::Shelf(9), &[shelf]), "Shelf");
    }
}
