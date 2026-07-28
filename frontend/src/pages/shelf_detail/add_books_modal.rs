//! The "Add books" modal: search + pick from the whole library, then append
//! the picked uuids to a manual shelf. Shared by both the web and mobile
//! shelf-detail surfaces.

use dioxus::prelude::*;
use omnibus_shared::EbookMetadata;

use crate::components::library_picker_grid::{filter_library, use_library_fetch};
use crate::components::LibraryPickerGrid;
use crate::{data, use_server_url};

/// Modal that appends library books to an existing manual shelf.
#[component]
pub(super) fn AddBooksModal(
    shelf_id: i64,
    on_close: EventHandler<()>,
    on_added: EventHandler<()>,
) -> Element {
    let server_url = use_server_url();
    let library = use_signal(Vec::<EbookMetadata>::new);
    let mut query = use_signal(String::new);
    let picked = use_signal(Vec::<String>::new);
    let mut saving = use_signal(|| false);

    use_library_fetch(server_url.clone(), library);

    let add_url = server_url.clone();
    let on_add = move |_| {
        if saving() || picked.read().is_empty() {
            return;
        }
        let url = add_url.clone();
        let uuids = picked.read().clone();
        let on_added = on_added;
        saving.set(true);
        spawn(async move {
            if data::add_shelf_books(&url, shelf_id, uuids).await.is_ok() {
                on_added.call(());
            }
            saving.set(false);
        });
    };

    // Memoized so filter only reruns when library/query change, not on every render.
    let filtered = use_memo(move || {
        let library_books = library.read();
        filter_library(&library_books, &query.read())
            .into_iter()
            .cloned()
            .collect::<Vec<EbookMetadata>>()
    });
    let filtered = filtered();
    let picked_count = picked.read().len();

    rsx! {
        div {
            class: "shelf-modal-overlay",
            "data-testid": "add-books-modal",
            onclick: move |_| on_close.call(()),
            div {
                class: "shelf-modal-card",
                onclick: move |e| e.stop_propagation(),
                div { class: "shelf-modal-body",
                    div { class: "shelf-picker-bar",
                        input {
                            r#type: "search",
                            class: "shelf-picker-search",
                            placeholder: "Search your library\u{2026}",
                            "data-testid": "add-books-search",
                            value: "{query}",
                            oninput: move |e| query.set(e.value()),
                        }
                        span { class: "mono shelf-picker-count", "Selected \u{b7} {picked_count}" }
                    }
                    LibraryPickerGrid { books: filtered, server_url: server_url.clone(), picked }
                }
                div { class: "shelf-modal-foot",
                    button {
                        r#type: "button", class: "btn shelf-btn-ghost",
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        r#type: "button", class: "btn shelf-btn-primary",
                        "data-testid": "add-books-submit",
                        disabled: saving(),
                        onclick: on_add,
                        if saving() { "Adding\u{2026}" } else { "Add \u{b7} {picked_count}" }
                    }
                }
            }
        }
    }
}
