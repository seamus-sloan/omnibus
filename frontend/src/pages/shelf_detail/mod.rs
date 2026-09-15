//! Shelf detail page (`/shelves/:id`). Resolves the shelf and its member
//! books, then hands both to the target's surface — on web the hero, action
//! bar, and member grid in [`body`] / [`header`]; in the native shell the
//! home-style screen in [`mobile`] — and mounts the add-books and edit-shelf
//! modals both share.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{EbookMetadata, Shelf, SortDir, SortKey};

use crate::components::{EditShelfModal, PageLoading};
use crate::{data, use_server_url, Route};

mod add_books_modal;
#[cfg(not(feature = "mobile"))]
mod body;
#[cfg(not(feature = "mobile"))]
mod header;
#[cfg(feature = "mobile")]
mod mobile;

use add_books_modal::AddBooksModal;
#[cfg(not(feature = "mobile"))]
use body::{back_crumb, ShelfBodySignals, WebShelfBody};

/// Shelf detail page — see the module doc.
#[component]
pub fn ShelfDetailPage(id: i64) -> Element {
    let server_url = use_server_url();
    let shelf = use_signal(|| None::<Shelf>);
    let books = use_signal(Vec::<EbookMetadata>::new);
    let loading = use_signal(|| true);
    let error = use_signal(|| None::<String>);
    crate::use_page_title(move || shelf.read().as_ref().map(|s| s.name.clone()));
    let sort_key = use_signal(|| SortKey::RecentlyInteracted);
    let show_add = use_signal(|| false);
    let edit_shelf = use_signal(|| false);
    // Bumped to force a refetch after a membership edit.
    let reload = use_signal(|| 0u32);
    // Set when the member-books refetch fails, so a transient network error
    // renders distinctly from a shelf that is genuinely empty (mirrors
    // `search_mobile.rs`'s `errored` signal for the same failure class).
    let errored = use_signal(|| false);

    use_shelf_effects(
        id,
        server_url.clone(),
        sort_key,
        reload,
        ShelfFetchSignals {
            shelf,
            loading,
            error,
            books,
            errored,
        },
    );

    if loading() && shelf.read().is_none() {
        return render_page_state(rsx! { PageLoading {} });
    }

    let Some(current) = shelf.read().clone() else {
        return render_page_state(rsx! {
            p { role: "alert", class: "subtitle",
                {error().unwrap_or_else(|| "Shelf not found.".into())}
            }
            Link { to: Route::Shelves {}, class: "btn", "Back to shelves" }
        });
    };

    let body = shelf_detail_body(
        &current,
        &books.read(),
        errored(),
        &server_url,
        ShelfUi {
            sort_key,
            show_add,
            edit_shelf,
            reload,
        },
    );

    // The add-books picker marks what the shelf already holds, so it needs the
    // current membership rather than discovering it by a failed add.
    let members: Vec<String> = books
        .read()
        .iter()
        .filter_map(|b| b.unique_identifier.clone())
        .collect();

    rsx! {
        {body}
        {shelf_detail_modals(current, members, show_add, edit_shelf, reload)}
    }
}

/// The page's UI-state signals, handed to whichever surface renders it.
/// `Copy` (Dioxus signals). Mobile drives only the two modal toggles.
#[derive(Clone, Copy)]
#[cfg_attr(feature = "mobile", allow(dead_code))]
struct ShelfUi {
    sort_key: Signal<SortKey>,
    show_add: Signal<bool>,
    edit_shelf: Signal<bool>,
    reload: Signal<u32>,
}

/// Web renders the hero + member grid; mobile renders the home-style
/// full-screen surface. Both consume the shared fetch pipeline in
/// [`use_shelf_effects`]. (Mobile is a separate build — rule 07 hydration
/// parity is unaffected.)
fn shelf_detail_body(
    current: &Shelf,
    books: &[EbookMetadata],
    errored: bool,
    server_url: &str,
    ui: ShelfUi,
) -> Element {
    #[cfg(feature = "mobile")]
    {
        let ShelfUi {
            mut show_add,
            mut edit_shelf,
            ..
        } = ui;
        rsx! {
            mobile::MobileShelfDetail {
                shelf: current.clone(),
                books: books.to_vec(),
                errored,
                server_url: server_url.to_string(),
                on_add: move |_| show_add.set(true),
                on_edit: move |_| edit_shelf.set(true),
            }
        }
    }
    #[cfg(not(feature = "mobile"))]
    {
        let ShelfUi {
            sort_key,
            show_add,
            edit_shelf,
            reload,
        } = ui;
        rsx! {
            WebShelfBody {
                shelf: current.clone(),
                books: books.to_vec(),
                errored,
                server_url: server_url.to_string(),
                signals: ShelfBodySignals {
                    sort_key,
                    show_add,
                    edit_shelf,
                    reload,
                },
            }
        }
    }
}

/// The "Add books" and "Edit shelf" modals, shown when their respective
/// signals flip true; both bump `reload` on success so the parent refetches.
fn shelf_detail_modals(
    current: Shelf,
    members: Vec<String>,
    mut show_add: Signal<bool>,
    mut edit_shelf: Signal<bool>,
    mut reload: Signal<u32>,
) -> Element {
    let shelf_id = current.id;
    let shelf_name = current.name.clone();
    rsx! {
        if show_add() {
            AddBooksModal {
                shelf_id,
                shelf_name,
                members,
                on_close: move |_| show_add.set(false),
                on_added: move |_| {
                    show_add.set(false);
                    reload.with_mut(|n| *n += 1);
                },
            }
        }

        if edit_shelf() {
            EditShelfModal {
                shelf: current.clone(),
                on_close: move |_| edit_shelf.set(false),
                on_saved: move |_| {
                    edit_shelf.set(false);
                    reload.with_mut(|n| *n += 1);
                },
            }
        }
    }
}

/// Data signals populated by the shelf detail fetch effects. `Copy` (Dioxus
/// signals), so grouping them keeps [`use_shelf_effects`] under the
/// too-many-arguments cap without changing call-site ergonomics.
#[derive(Clone, Copy)]
struct ShelfFetchSignals {
    shelf: Signal<Option<Shelf>>,
    loading: Signal<bool>,
    error: Signal<Option<String>>,
    books: Signal<Vec<EbookMetadata>>,
    errored: Signal<bool>,
}

/// Wires the two data-fetch effects backing [`ShelfDetailPage`]: the shelf
/// detail itself (id-driven) and its member books (id/sort/reload-driven).
/// Extracted to keep the page component under the line cap; mirrors
/// `book_detail::use_book_data_effects`.
fn use_shelf_effects(
    id: i64,
    server_url: String,
    sort_key: Signal<SortKey>,
    reload: Signal<u32>,
    sig: ShelfFetchSignals,
) {
    let ShelfFetchSignals {
        mut shelf,
        mut loading,
        mut error,
        mut books,
        mut errored,
    } = sig;

    // Fetch the shelf detail whenever the id changes. `id` is a plain prop
    // (not a signal), so it must be wrapped in `use_reactive!` to re-arm this
    // effect on navigation between shelves — see `BookDetailPage` for why.
    let shelf_url = server_url.clone();
    let generation = crate::use_cache_generation();
    use_effect(use_reactive!(|id| {
        let url = shelf_url.clone();
        let _ = reload();
        // Re-run on cache-revalidation bumps; the refetch is a cache hit.
        let _ = generation();
        spawn(async move {
            let same_shelf = shelf.peek().as_ref().map(|s| s.id) == Some(id);
            if !same_shelf {
                loading.set(true);
            }
            match data::get_shelf(&url, id).await {
                Ok(s) => {
                    shelf.set(Some(s));
                    error.set(None);
                }
                Err(e) => {
                    shelf.set(None);
                    error.set(Some(e.to_string()));
                }
            }
            loading.set(false);
        });
    }));

    // Fetch the member books, re-running on id/sort change or a membership
    // edit. `sort_key()` is a signal read, already tracked; `id` needs the
    // same `use_reactive!` wrapping as above.
    let page_url = server_url;
    use_effect(use_reactive!(|id| {
        let url = page_url.clone();
        let key = sort_key();
        let _ = reload();
        // Re-run on cache-revalidation bumps; the refetch is a cache hit.
        let _ = generation();
        spawn(async move {
            let dir = default_dir_for(key);
            match data::shelf_page(&url, id, key, dir).await {
                Ok(page) => {
                    books.set(page.books);
                    errored.set(false);
                }
                Err(_) => {
                    // `tracing` isn't linked under the `web` (WASM) feature,
                    // so the signal alone carries the failure to the UI.
                    // Clear the stale list too — otherwise a refetch failure
                    // after navigating shelves leaves the prior shelf's
                    // books on screen under the error banner.
                    books.set(Vec::new());
                    errored.set(true);
                }
            }
        });
    }));
}

/// Loading / not-found chrome. Web keeps the way back to the index above it;
/// mobile renders the bare screen surface.
fn render_page_state(inner: Element) -> Element {
    #[cfg(not(feature = "mobile"))]
    {
        rsx! {
            div { class: "shd-page",
                {back_crumb()}
                div { class: "shd-state", {inner} }
            }
        }
    }
    #[cfg(feature = "mobile")]
    {
        rsx! {
            div { class: "m-shelves", {inner} }
        }
    }
}

/// The default sort direction the grid uses for a given sort axis: newest-first
/// for the date axes, ascending otherwise.
fn default_dir_for(key: SortKey) -> SortDir {
    match key {
        SortKey::NewestAdded | SortKey::LastUpdated | SortKey::RecentlyInteracted => SortDir::Desc,
        _ => SortDir::Asc,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_dir_for_dates_is_desc() {
        assert_eq!(default_dir_for(SortKey::NewestAdded), SortDir::Desc);
        assert_eq!(default_dir_for(SortKey::LastUpdated), SortDir::Desc);
        assert_eq!(default_dir_for(SortKey::RecentlyInteracted), SortDir::Desc);
        assert_eq!(default_dir_for(SortKey::Title), SortDir::Asc);
    }
}
