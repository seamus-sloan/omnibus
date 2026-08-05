//! Owns [`LandingSignals`] — every reactive signal the landing page reads —
//! and [`setup_landing_signals`], which constructs them and arms the
//! fetch/hydration effects via [`wire_landing_effects`]. Kept apart from
//! [`super::view`] (derived per-render state) and [`super::body`]
//! (presentation) so each stays a focused stage of the page's pipeline.

use std::collections::BTreeSet;

use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, ResumePoint, Shelf, ShelfSummary, ViewPrefs};

#[cfg(feature = "web")]
use super::effects::spawn_load_more_observer;
use super::effects::{
    spawn_hero_effect, spawn_load_more_effect, spawn_page_fetch_effect,
    spawn_selected_shelf_effect, spawn_shelf_books_effect, spawn_shelves_list_effect,
    spawn_suggestion_pools_effect, FetchSignals, ShelfFetchSignals, SuggestionPools,
};
use crate::components::chip_editor::SuggestionItem;
use crate::shelf_selection::{self, ShelfSelection};
use crate::view_prefs;

/// Owned handles for every reactive signal the landing page wires up.
/// Returned by [`setup_landing_signals`] so [`super::LandingPage`] can stay a
/// thin composition of named stages.
#[derive(Copy, Clone)]
// Mobile reads a subset of these (it renders a compact grid, no admin table or
// suggestion pools); the rest feed the web rail/toolbar only.
#[cfg_attr(feature = "mobile", allow(dead_code))]
pub(super) struct LandingSignals {
    pub(super) books: Signal<Vec<EbookMetadata>>,
    pub(super) next_cursor: Signal<Option<String>>,
    pub(super) total: Signal<Option<i64>>,
    pub(super) lib_path: Signal<Option<String>>,
    pub(super) lib_error: Signal<Option<String>>,
    pub(super) loading: Signal<bool>,
    pub(super) loading_more: Signal<bool>,
    pub(super) error: Signal<Option<String>>,
    pub(super) prefs: Signal<ViewPrefs>,
    pub(super) want_more: Signal<u32>,
    pub(super) is_admin: ReadSignal<bool>,
    pub(super) pools: SuggestionPools,
    /// Which lens the book list shows: All Books or one shelf (gallery pick).
    pub(super) selection: Signal<ShelfSelection>,
    /// Gallery feed. Starts empty so the first WASM paint matches SSR.
    pub(super) shelves: Signal<Vec<ShelfSummary>>,
    /// Bumped after a shelf create so the gallery refetches its list.
    pub(super) shelves_tick: Signal<u32>,
    /// Selected shelf's member list (`None` = All Books / not yet loaded).
    pub(super) shelf_books: Signal<Option<Vec<EbookMetadata>>>,
    pub(super) shelf_loading: Signal<bool>,
    pub(super) shelf_error: Signal<Option<String>>,
    /// Full detail for the selected shelf (`None` on All Books / while it
    /// loads) — feeds the header facet row and the edit-shelf modal.
    pub(super) selected_shelf: Signal<Option<Shelf>>,
    /// True while the edit-shelf modal is open.
    pub(super) edit_shelf: Signal<bool>,
    /// Continue-reading hero feed. Starts empty (hero hidden) for SSR parity.
    pub(super) hero_points: Signal<Vec<ResumePoint>>,
    /// Table-view bulk-edit selection: the checked rows' uuids. Cleared
    /// whenever the visible list changes wholesale (refetch or shelf pick).
    pub(super) bulk_selected: Signal<BTreeSet<String>>,
    /// True while the bulk-edit modal is open.
    pub(super) bulk_modal_open: Signal<bool>,
}

/// Construct every signal the landing page owns and arm the effects that
/// fetch data into them. Must run from inside [`super::LandingPage`] — every
/// call here is a Dioxus hook, so the call order is stable across renders.
pub(super) fn setup_landing_signals(server_url: &str, query: Signal<String>) -> LandingSignals {
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
    // Suggestion pools for the inline Authors, Tags, and Genres chip
    // editors, each carrying the dropdown book-count.
    let pools = SuggestionPools {
        authors: use_signal(Vec::<SuggestionItem>::new),
        tags: use_signal(Vec::<SuggestionItem>::new),
        genres: use_signal(Vec::<SuggestionItem>::new),
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
    let bulk_selected = use_signal(BTreeSet::<String>::new);
    let bulk_modal_open = use_signal(|| false);
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
        bulk_selected,
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
        bulk_selected,
        bulk_modal_open,
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
    mut bulk_selected: Signal<BTreeSet<String>>,
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

    // Drop the bulk-edit selection whenever the visible list changes
    // wholesale — a refetch (query/sort/filter change) or a gallery pick.
    // Checked rows that are no longer rendered would otherwise be edited
    // invisibly from a stale selection.
    let selection_for_bulk = shelf_wiring.selection;
    use_effect(move || {
        let _ = fetch_key();
        let _ = selection_for_bulk();
        if !bulk_selected.peek().is_empty() {
            bulk_selected.write().clear();
        }
    });

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
