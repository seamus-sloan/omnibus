//! Derives the per-render [`LandingViewState`] snapshot and the
//! [`LandingHandlers`] event-handler bundle from [`super::signals::LandingSignals`]
//! — the "what does this render show, and what does clicking it do" stage
//! between signal wiring ([`super::signals`]) and presentation
//! ([`super::body`]).

use dioxus::prelude::*;
use omnibus_shared::{EbookMetadata, ShelfSummary, ViewFilters, ViewPrefs};

use super::filtering::apply_filters;
use super::signals::LandingSignals;
use super::sorting::sort_books;
use crate::shelf_selection::{self, ShelfSelection};
use crate::view_prefs;

/// Which list feeds the grid/table. Search always wins (the palette overlays
/// everything); a gallery pick overlays browse; browse is the default.
#[derive(Copy, Clone, PartialEq, Eq, Debug)]
pub(super) enum VisibleSource {
    Search,
    Shelf,
    Browse,
}

/// Resolve the precedence search > shelf > browse for this render.
pub(super) fn visible_source(is_search: bool, selection: ShelfSelection) -> VisibleSource {
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
pub(super) fn section_title(selection: ShelfSelection, shelves: &[ShelfSummary]) -> String {
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
/// Computed by [`derive_view_state`] so the [`super::LandingPage`] body is
/// just composition.
#[cfg_attr(feature = "mobile", allow(dead_code))]
pub(super) struct LandingViewState {
    pub(super) is_loading: bool,
    pub(super) page_error: Option<String>,
    pub(super) lib_err: Option<String>,
    pub(super) path_subtitle: String,
    pub(super) path_missing: bool,
    pub(super) book_count: usize,
    /// The "N hidden" receipt beside the browse header count; `None` off the
    /// browse lens or when the viewer hides nothing.
    pub(super) hidden_count: Option<i64>,
    pub(super) visible_books: Vec<EbookMetadata>,
    pub(super) visible_is_empty: bool,
    pub(super) books_empty: bool,
    pub(super) has_more: bool,
    pub(super) is_loading_more: bool,
    pub(super) section_title: String,
    /// Remount key for the book area: changing it replays the sweep-in
    /// cascade (a fresh subtree restarts its CSS animations).
    pub(super) sweep_key: String,
    /// True while the palette query is non-empty — hero + gallery hide, and
    /// the search result set renders exactly as before the redesign.
    pub(super) is_search: bool,
}

/// Snapshot every signal the markup needs in one place. Reads are cheap, but
/// doing them inline in `rsx!` would multiply each `prefs()`/`books()` call
/// across the three child components.
pub(super) fn derive_view_state(sigs: &LandingSignals, query: Signal<String>) -> LandingViewState {
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
        .map(|p| super::short_path(p))
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
        // The exclusion (and so the receipt) applies to All Books only.
        hidden_count: match source {
            VisibleSource::Browse => (sigs.hidden)(),
            _ => None,
        },
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
/// [`super::LandingPage`] body stays a thin composition.
#[cfg_attr(feature = "mobile", allow(dead_code))]
pub(super) struct LandingHandlers {
    pub(super) on_prefs_change_header: EventHandler<ViewPrefs>,
    pub(super) on_prefs_change_content: EventHandler<ViewPrefs>,
    pub(super) on_load_more: EventHandler<()>,
    pub(super) on_clear_filters: EventHandler<()>,
    /// Gallery pick: move the glow, persist the choice, swap the book list.
    pub(super) on_select_shelf: EventHandler<ShelfSelection>,
    /// After a create in the gallery's modal: refetch the shelves list.
    pub(super) on_shelf_created: EventHandler<()>,
}

/// Build the UI-event handlers from the landing signals. `save` is `Copy`
/// because every capture (`prefs`, `lib_path` — both `Signal`) is `Copy`,
/// so each handler can take its own reference to the same persisted-prefs
/// update path without cloning closure state.
pub(super) fn build_handlers(sigs: &LandingSignals) -> LandingHandlers {
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
