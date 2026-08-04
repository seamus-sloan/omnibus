//! Presentation branches of [`super::LandingPage`]: the mobile compact-grid
//! body and the web hero/gallery/header/content/bulk-edit body, plus the
//! bulk-edit list-merge helpers they share with the modal's save handler.

#[cfg(not(feature = "mobile"))]
use std::collections::BTreeSet;

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use omnibus_shared::EbookMetadata;

#[cfg(not(feature = "mobile"))]
use super::bulk_edit;
#[cfg(not(feature = "mobile"))]
use super::hero::ContinueHero;
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
    let mut edit_shelf = sigs.edit_shelf;
    let mut shelves_tick = sigs.shelves_tick;
    let mut bulk_selected = sigs.bulk_selected;
    let mut bulk_modal_open = sigs.bulk_modal_open;
    let bulk_count = bulk_selected.read().len();
    let show_bulk_bar = (sigs.is_admin)() && bulk_count > 0;
    // Snapshot the selection for the modal only while it's open — the
    // backdrop blocks further checkbox toggles, so this stays stable for
    // the modal's lifetime.
    let (bulk_uuids, bulk_books) = if bulk_modal_open() {
        let set = bulk_selected.read();
        (
            set.iter().cloned().collect::<Vec<String>>(),
            selected_bulk_books(&set, &sigs.books.read(), sigs.shelf_books.read().as_deref()),
        )
    } else {
        (Vec::new(), Vec::new())
    };
    let mut books_sig = sigs.books;
    let mut shelf_books_sig = sigs.shelf_books;
    // Annotated outside the rsx props — a bare `.into()` there is ambiguous
    // between the dioxus_core and dioxus_stores `SuperInto` impls.
    let author_pool: ReadSignal<Vec<SuggestionItem>> = sigs.pools.authors.into();
    let tag_pool: ReadSignal<Vec<SuggestionItem>> = sigs.pools.tags.into();
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
                        selected: bulk_selected,
                    },
                    handlers: LandingContentHandlers {
                        on_prefs_change: on_prefs_change_content,
                        on_load_more,
                        on_clear_filters,
                    },
                    sweep_key: view.sweep_key,
                }
            }

            if show_bulk_bar {
                bulk_edit::BulkEditBar {
                    count: bulk_count,
                    on_edit: move |_| bulk_modal_open.set(true),
                    on_clear: move |_| bulk_selected.write().clear(),
                }
            }
            if bulk_modal_open() {
                bulk_edit::BulkEditModal {
                    uuids: bulk_uuids,
                    selected_books: bulk_books,
                    suggestions: bulk_edit::BulkEditSuggestions {
                        author_suggestions: author_pool,
                        tag_suggestions: tag_pool,
                    },
                    on_close: move |_| bulk_modal_open.set(false),
                    on_saved: move |updated: Vec<EbookMetadata>| {
                        install_updated_books(&mut books_sig, &mut shelf_books_sig, &updated);
                        bulk_selected.write().clear();
                        bulk_modal_open.set(false);
                    },
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
