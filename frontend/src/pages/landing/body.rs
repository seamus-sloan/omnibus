//! Presentation branches of [`super::LandingPage`]: the mobile compact-grid
//! body and the web marquee body — the stack, the shelves row, the section
//! header, the cover wall, and the edge resume ribbon — plus the bulk-edit
//! list-merge helpers they share with the modal's save handler.

#[cfg(not(feature = "mobile"))]
use std::collections::BTreeSet;

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use omnibus_shared::EbookMetadata;

#[cfg(not(feature = "mobile"))]
use super::bulk_edit;
#[cfg(feature = "mobile")]
use super::mobile;
#[cfg(feature = "mobile")]
use super::pull_refresh;
#[cfg(not(feature = "mobile"))]
use super::sections::{
    BooksView, LandingContent, LandingContentHandlers, LandingContentProps, LandingHeader,
    LandingHeaderView,
};
#[cfg(not(feature = "mobile"))]
use super::shelf_gallery::ShelfGallery;
use super::signals::LandingSignals;
#[cfg(not(feature = "mobile"))]
use super::stack::{lead_accent_style, stack_entries, EdgeResume, ResumeStack};
#[cfg(not(feature = "mobile"))]
use super::table::BookTableContext;
use super::view::{LandingHandlers, LandingViewState};
#[cfg(not(feature = "mobile"))]
use crate::components::chip_editor::SuggestionItem;
#[cfg(not(feature = "mobile"))]
use crate::components::EditShelfModal;

/// Mobile presentation branch of [`super::LandingPage`]: a single compact
/// grid plus the continue card and sort & filter sheet — no admin table or
/// suggestion pools.
#[cfg(feature = "mobile")]
pub(super) fn mobile_landing_body(
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
            hidden_count: view.hidden_count,
            books: view.visible_books,
            paging: mobile::MobileLandingPaging {
                is_loading: view.is_loading,
                has_more: view.has_more,
                is_loading_more: view.is_loading_more,
                on_load_more: handlers.on_load_more,
            },
            prefs: (sigs.prefs)(),
            on_prefs_change: handlers.on_prefs_change_header,
            server_url,
        }
    }
}

/// Web presentation branch of [`super::LandingPage`]: continue-reading hero +
/// shelf gallery + section header + grid/table content, wired to the admin
/// table context and suggestion pools. The gallery filters in place — the old
/// shelves rail (still used on `/shelves/:id`) no longer mounts here.
#[cfg(not(feature = "mobile"))]
pub(super) fn web_landing_body(
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
    let edit_shelf = sigs.edit_shelf;
    let shelves_tick = sigs.shelves_tick;
    let bulk_selected = sigs.bulk_selected;
    let bulk_modal_open = sigs.bulk_modal_open;
    let bulk = snapshot_bulk_selection(sigs, (sigs.is_admin)(), bulk_modal_open());
    let books_sig = sigs.books;
    let shelf_books_sig = sigs.shelf_books;
    // Annotated outside the rsx props — a bare `.into()` there is ambiguous
    // between the dioxus_core and dioxus_stores `SuperInto` impls.
    let author_pool: ReadSignal<Vec<SuggestionItem>> = sigs.pools.authors.into();
    let tag_pool: ReadSignal<Vec<SuggestionItem>> = sigs.pools.tags.into();
    let genre_pool: ReadSignal<Vec<SuggestionItem>> = sigs.pools.genres.into();
    let all_cover_uuids = derive_all_cover_uuids(sigs);
    // Which open book the stack has out front. The whole page takes its
    // accent, and the edge ribbon resumes it — so it is owned here rather
    // than inside either of them. Hooks stay unconditional: this helper is on
    // the web build's only render path.
    let lead = use_signal(|| 0usize);
    // Read (not held) so a cover edit elsewhere re-renders the stack with a
    // fresh thumb URL, without carrying a signal guard across the rsx below.
    let cover_bust = crate::contexts::use_cover_cache_bust().0.read().clone();
    let entries = stack_entries(&hero_points, &server_url, &cover_bust);
    let accent_style = lead_accent_style(&entries, lead());
    let show_stack = !view.is_search && !entries.is_empty();
    // The glue binds elements the shelves row and the stack only mount once
    // their fetches land, so it re-runs when either appears or disappears.
    let glue_key = (show_stack, !view.is_search, (sigs.shelves)().len());
    use_effect(use_reactive!(|glue_key| {
        let _ = glue_key;
        let _ = dioxus::document::eval(MARQUEE_JS);
    }));
    rsx! {
        div { class: "landing-col lmq", id: "lmq-root", style: "{accent_style}",
            if show_stack {
                ResumeStack { entries: entries.clone(), lead }
            }
            {render_gallery(sigs, view.is_search, all_cover_uuids, server_url.clone(), on_select_shelf, on_shelf_created)}
            {render_header_and_content(sigs, view, prefs(), server_url, selected_shelf.clone(), edit_shelf, bulk_selected, LandingContentHandlers { on_prefs_change: on_prefs_change_content, on_load_more, on_clear_filters }, on_prefs_change_header)}
            {render_bulk_overlay(bulk, bulk_modal_open, bulk_selected, books_sig, shelf_books_sig, author_pool, tag_pool, genre_pool)}
            {render_edit_shelf_overlay(edit_shelf, selected_shelf, shelves_tick)}
            if show_stack {
                EdgeResume { entries, lead }
                div { class: "lmq-scrollhint", aria_hidden: true,
                    "scroll \u{2014} the books take over"
                    span { class: "arr", "\u{2193}" }
                }
            }
        }
    }
}

/// Scroll + shelves-row paging glue, installed post-mount so SSR and the
/// first WASM paint render identical markup (rule 07).
#[cfg(not(feature = "mobile"))]
const MARQUEE_JS: &str = include_str!("marquee.js");

/// Section header + grid/table content for [`web_landing_body`]. See
/// [`render_bulk_overlay`] for why this is a plain helper, not a component.
#[cfg(not(feature = "mobile"))]
#[allow(clippy::too_many_arguments)]
fn render_header_and_content(
    sigs: &LandingSignals,
    view: LandingViewState,
    prefs: omnibus_shared::ViewPrefs,
    server_url: String,
    selected_shelf: Option<omnibus_shared::Shelf>,
    mut edit_shelf: Signal<bool>,
    bulk_selected: Signal<BTreeSet<String>>,
    content_handlers: LandingContentHandlers,
    on_prefs_change_header: EventHandler<omnibus_shared::ViewPrefs>,
) -> Element {
    rsx! {
        LandingHeader {
            view: LandingHeaderView {
                path_subtitle: view.path_subtitle,
                book_count: view.book_count,
                hidden_count: view.hidden_count,
                path_missing: view.path_missing,
                page_error: view.page_error.clone(),
                lib_err: view.lib_err.clone(),
                section_title: view.section_title,
                selected_shelf,
            },
            prefs: prefs.clone(),
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
                prefs,
                ctx: BookTableContext {
                    server_url,
                    is_admin: (sigs.is_admin)(),
                    author_suggestions: sigs.pools.authors.into(),
                    tag_suggestions: sigs.pools.tags.into(),
                    genre_suggestions: sigs.pools.genres.into(),
                    selected: bulk_selected,
                },
                handlers: content_handlers,
                sweep_key: view.sweep_key,
            }
        }
    }
}

/// The shelves row for [`web_landing_body`], hidden entirely while a search
/// is active. See [`render_bulk_overlay`] for why this is a plain helper, not
/// a component.
#[cfg(not(feature = "mobile"))]
fn render_gallery(
    sigs: &LandingSignals,
    is_search: bool,
    all_cover_uuids: Vec<String>,
    server_url: String,
    on_select_shelf: EventHandler<crate::shelf_selection::ShelfSelection>,
    on_shelf_created: EventHandler<()>,
) -> Element {
    rsx! {
        if !is_search {
            ShelfGallery {
                shelves: (sigs.shelves)(),
                selection: (sigs.selection)(),
                all_count: (sigs.total)(),
                all_cover_uuids,
                server_url,
                on_select: on_select_shelf,
                on_created: on_shelf_created,
            }
        }
    }
}

/// Bulk-edit bar + modal overlay for [`web_landing_body`], split out so the
/// parent's rsx! reads as one page layout rather than page-plus-modals. A
/// plain `Element`-returning helper (not a `#[component]`) since it shares
/// `web_landing_body`'s signals directly rather than re-deriving props.
#[cfg(not(feature = "mobile"))]
#[allow(clippy::too_many_arguments)]
fn render_bulk_overlay(
    bulk: BulkSelectionSnapshot,
    mut bulk_modal_open: Signal<bool>,
    mut bulk_selected: Signal<BTreeSet<String>>,
    mut books_sig: Signal<Vec<EbookMetadata>>,
    mut shelf_books_sig: Signal<Option<Vec<EbookMetadata>>>,
    author_pool: ReadSignal<Vec<SuggestionItem>>,
    tag_pool: ReadSignal<Vec<SuggestionItem>>,
    genre_pool: ReadSignal<Vec<SuggestionItem>>,
) -> Element {
    rsx! {
        if bulk.show_bar {
            bulk_edit::BulkEditBar {
                count: bulk.count,
                on_edit: move |_| bulk_modal_open.set(true),
                on_clear: move |_| bulk_selected.write().clear(),
            }
        }
        if bulk_modal_open() {
            bulk_edit::BulkEditModal {
                uuids: bulk.uuids,
                selected_books: bulk.books,
                suggestions: bulk_edit::BulkEditSuggestions {
                    author_suggestions: author_pool,
                    tag_suggestions: tag_pool,
                    genre_suggestions: genre_pool,
                },
                on_close: move |_| bulk_modal_open.set(false),
                on_saved: move |updated: Vec<EbookMetadata>| {
                    install_updated_books(&mut books_sig, &mut shelf_books_sig, &updated);
                    bulk_selected.write().clear();
                    bulk_modal_open.set(false);
                },
            }
        }
    }
}

/// Edit-shelf modal overlay for [`web_landing_body`] — see
/// [`render_bulk_overlay`] for why this is a plain helper, not a component.
#[cfg(not(feature = "mobile"))]
fn render_edit_shelf_overlay(
    mut edit_shelf: Signal<bool>,
    selected_shelf: Option<omnibus_shared::Shelf>,
    mut shelves_tick: Signal<u32>,
) -> Element {
    rsx! {
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

/// Derived bulk-selection state for [`web_landing_body`]: whether to show
/// the bulk-edit bar, plus the uuids/books the modal needs.
#[cfg(not(feature = "mobile"))]
struct BulkSelectionSnapshot {
    count: usize,
    show_bar: bool,
    uuids: Vec<String>,
    books: Vec<EbookMetadata>,
}

/// Snapshot the current bulk selection for [`web_landing_body`]. `uuids`/
/// `books` are populated only while the modal is open — the backdrop blocks
/// further checkbox toggles, so this stays stable for the modal's lifetime.
#[cfg(not(feature = "mobile"))]
fn snapshot_bulk_selection(
    sigs: &LandingSignals,
    is_admin: bool,
    modal_open: bool,
) -> BulkSelectionSnapshot {
    let bulk_selected = sigs.bulk_selected;
    let count = bulk_selected.read().len();
    let show_bar = is_admin && count > 0;
    let (uuids, books) = if modal_open {
        let set = bulk_selected.read();
        (
            set.iter().cloned().collect::<Vec<String>>(),
            selected_bulk_books(&set, &sigs.books.read(), sigs.shelf_books.read().as_deref()),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    BulkSelectionSnapshot {
        count,
        show_bar,
        uuids,
        books,
    }
}

/// All Books mosaic covers: first four cover-bearing books of the
/// (always-warm) browse page. Before it lands, the tile falls back to its
/// accent plate.
#[cfg(not(feature = "mobile"))]
fn derive_all_cover_uuids(sigs: &LandingSignals) -> Vec<String> {
    sigs.books
        .read()
        .iter()
        .filter(|b| b.cover_url.is_some())
        .filter_map(|b| b.unique_identifier.clone())
        .take(4)
        .collect()
}

/// Collect the selected books' current metadata from the browse list plus
/// (when a shelf lens is active) the shelf member list, deduped by uuid —
/// feeds the bulk-edit modal's remove-tag suggestions.
#[cfg(not(feature = "mobile"))]
pub(super) fn selected_bulk_books(
    selected: &BTreeSet<String>,
    books: &[EbookMetadata],
    shelf_books: Option<&[EbookMetadata]>,
) -> Vec<EbookMetadata> {
    let mut found: Vec<EbookMetadata> = Vec::with_capacity(selected.len());
    let picked = |b: &EbookMetadata| {
        b.unique_identifier
            .as_deref()
            .is_some_and(|u| selected.contains(u))
    };
    for book in books.iter().filter(|b| picked(b)) {
        found.push(book.clone());
    }
    for book in shelf_books.unwrap_or_default().iter().filter(|b| picked(b)) {
        if !found
            .iter()
            .any(|f| f.unique_identifier == book.unique_identifier)
        {
            found.push(book.clone());
        }
    }
    found
}

/// Replace, by uuid, every book in `list` that the bulk save returned. Pure
/// so it's testable without a Dioxus runtime.
#[cfg(not(feature = "mobile"))]
pub(super) fn replace_updated_books(list: &mut [EbookMetadata], updated: &[EbookMetadata]) {
    for book in list.iter_mut() {
        if let Some(fresh) = updated
            .iter()
            .find(|u| u.unique_identifier == book.unique_identifier)
        {
            *book = fresh.clone();
        }
    }
}

/// Install the bulk save's returned metadata into the browse and shelf list
/// signals — the same optimistic-install idiom the inline cell editors use,
/// so no refetch is needed.
#[cfg(not(feature = "mobile"))]
fn install_updated_books(
    books: &mut Signal<Vec<EbookMetadata>>,
    shelf_books: &mut Signal<Option<Vec<EbookMetadata>>>,
    updated: &[EbookMetadata],
) {
    books.with_mut(|list| replace_updated_books(list, updated));
    shelf_books.with_mut(|maybe| {
        if let Some(list) = maybe.as_mut() {
            replace_updated_books(list, updated);
        }
    });
}
