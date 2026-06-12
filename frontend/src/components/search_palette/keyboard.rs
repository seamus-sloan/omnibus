//! Builds the `onkeydown` handler for the palette panel. Arrow keys drive
//! a single index across the flat item list; Enter either opens the
//! highlighted row (after the user has engaged keyboard nav) or routes to
//! the full-page `/search/:query` view (when no row is highlighted yet).

use dioxus::prelude::*;

use super::model::{facet_query, FlatItem};
use super::PaletteOpen;
use crate::Route;

/// Everything the palette's keydown handler needs to read or mutate.
/// Bundled so [`make_keydown_handler`] doesn't take a long parameter
/// list; the fields are all `Copy` (Dioxus signals + the navigator)
/// so the struct is itself cheap to move into the returned closure.
pub(super) struct KeyboardContext {
    pub(super) open: PaletteOpen,
    pub(super) selected: Signal<usize>,
    pub(super) has_navigated: Signal<bool>,
    pub(super) flat_items: Memo<Vec<FlatItem>>,
    pub(super) query: Signal<String>,
    pub(super) nav: dioxus_router::Navigator,
}

/// Build the `onkeydown` event handler for the palette panel.
pub(super) fn make_keydown_handler(ctx: KeyboardContext) -> impl FnMut(Event<KeyboardData>) {
    let KeyboardContext {
        mut open,
        mut selected,
        mut has_navigated,
        flat_items,
        query,
        nav,
    } = ctx;
    move |evt: Event<KeyboardData>| {
        let key = evt.key();
        match key {
            Key::Escape => {
                evt.prevent_default();
                open.0.set(false);
            }
            Key::ArrowDown => {
                evt.prevent_default();
                let len = flat_items.read().len();
                if len > 0 {
                    // First arrow press is a "begin navigation" gesture — it
                    // should highlight the first item, not increment past it.
                    // Subsequent presses wrap around as usual.
                    if !has_navigated() {
                        selected.set(0);
                    } else {
                        selected.set((selected() + 1) % len);
                    }
                    has_navigated.set(true);
                }
            }
            Key::ArrowUp => {
                evt.prevent_default();
                let len = flat_items.read().len();
                if len > 0 {
                    // Symmetric with ArrowDown: first press lands on the
                    // last item; subsequent presses walk backward.
                    if !has_navigated() {
                        selected.set(len - 1);
                    } else {
                        selected.set(if selected() == 0 {
                            len - 1
                        } else {
                            selected() - 1
                        });
                    }
                    has_navigated.set(true);
                }
            }
            Key::Enter => {
                evt.prevent_default();
                let items = flat_items.read();
                if has_navigated() {
                    // Navigate to the selected result. Books/Authors/Series go
                    // to their detail pages; Tags fall through to /search (no
                    // per-tag detail page exists yet).
                    if let Some(item) = items.get(selected()) {
                        match item {
                            FlatItem::Book { uuid, .. } => {
                                nav.push(Route::BookDetail { uuid: uuid.clone() });
                            }
                            FlatItem::Author { id, .. } => {
                                nav.push(Route::AuthorDetail { id: *id });
                            }
                            FlatItem::Series { id, .. } => {
                                nav.push(Route::SeriesDetail { id: *id });
                            }
                            FlatItem::Tag { name, .. } => {
                                nav.push(Route::Search {
                                    query: facet_query("tag", name),
                                });
                            }
                        }
                        open.0.set(false);
                    }
                } else {
                    let q = query().trim().to_string();
                    if !q.is_empty() {
                        nav.push(Route::Search { query: q });
                        open.0.set(false);
                    }
                }
            }
            _ => {}
        }
    }
}
