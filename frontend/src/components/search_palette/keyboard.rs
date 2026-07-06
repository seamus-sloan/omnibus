//! Builds the `onkeydown` handler for the palette panel. Arrow keys drive
//! a single index across the flat item list; Enter either opens the
//! highlighted row (after the user has engaged keyboard nav) or routes to
//! the full-page `/search/:query` view (when no row is highlighted yet).

use dioxus::prelude::*;

use super::model::{facet_query, FlatItem};
use super::PaletteOpen;
use crate::Route;

/// Inputs read or mutated by the palette's keydown handler.
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
        selected,
        has_navigated,
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
                move_selection(selected, has_navigated, flat_items, Direction::Down);
            }
            Key::ArrowUp => {
                evt.prevent_default();
                move_selection(selected, has_navigated, flat_items, Direction::Up);
            }
            Key::Enter => {
                evt.prevent_default();
                handle_enter(open, selected, has_navigated, flat_items, query, &nav);
            }
            _ => {}
        }
    }
}

/// Arrow-key direction for [`move_selection`].
#[derive(Copy, Clone)]
enum Direction {
    Up,
    Down,
}

/// Move the highlight one step in `dir`, wrapping. The first arrow press is a
/// "begin navigation" gesture: it lands on the first (Down) or last (Up) item
/// rather than stepping past it; subsequent presses wrap around as usual.
fn move_selection(
    mut selected: Signal<usize>,
    mut has_navigated: Signal<bool>,
    flat_items: Memo<Vec<FlatItem>>,
    dir: Direction,
) {
    let len = flat_items.read().len();
    if len == 0 {
        return;
    }
    let next = match (dir, has_navigated()) {
        (Direction::Down, false) => 0,
        (Direction::Down, true) => (selected() + 1) % len,
        (Direction::Up, false) => len - 1,
        (Direction::Up, true) if selected() == 0 => len - 1,
        (Direction::Up, true) => selected() - 1,
    };
    selected.set(next);
    has_navigated.set(true);
}

/// Handle Enter: open the highlighted row once the user has engaged keyboard
/// nav (Books/Authors/Series to their detail pages; Tags fall through to
/// `/search`), else route to the full-page `/search/:query` view.
fn handle_enter(
    mut open: PaletteOpen,
    selected: Signal<usize>,
    has_navigated: Signal<bool>,
    flat_items: Memo<Vec<FlatItem>>,
    query: Signal<String>,
    nav: &dioxus_router::Navigator,
) {
    let items = flat_items.read();
    if has_navigated() {
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
