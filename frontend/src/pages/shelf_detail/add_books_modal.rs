//! The "Add books" modal: search the library, pick books, and append them to
//! a hand-picked shelf. It names the shelf it is adding to, marks the books
//! already on it, says how many it will add, and on a failure keeps everything
//! and says why. Shared by the web and mobile shelf-detail surfaces.

use dioxus::prelude::*;

use crate::components::LibraryPicker;
use crate::{data, use_server_url};

/// Modal that appends library books to an existing manual shelf. `members`
/// are the uuids the shelf already holds; `shelf_name` titles the dialog.
#[component]
pub(super) fn AddBooksModal(
    shelf_id: i64,
    shelf_name: String,
    members: Vec<String>,
    on_close: EventHandler<()>,
    on_added: EventHandler<()>,
) -> Element {
    let server_url = use_server_url();
    let picked = use_signal(Vec::<String>::new);
    let mut saving = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);

    let add_url = server_url.clone();
    let on_add = move |_| {
        if saving() || picked.read().is_empty() {
            return;
        }
        let url = add_url.clone();
        let uuids = picked.read().clone();
        saving.set(true);
        error.set(None);
        spawn(async move {
            match data::add_shelf_books(&url, shelf_id, uuids).await {
                Ok(()) => on_added.call(()),
                Err(e) => error.set(Some(e.to_string())),
            }
            saving.set(false);
        });
    };

    let count = picked.read().len();
    let add_label = match count {
        0 => "Add books".to_string(),
        1 => "Add 1 book".to_string(),
        n => format!("Add {n} books"),
    };

    rsx! {
        div {
            class: "shelf-modal-overlay",
            "data-testid": "add-books-modal",
            onclick: move |_| on_close.call(()),
            div {
                class: "shelf-modal-card",
                role: "dialog",
                "aria-modal": "true",
                "aria-labelledby": "add-books-title",
                tabindex: "-1",
                onclick: move |e| e.stop_propagation(),
                onkeydown: move |e| {
                    if e.key() == Key::Escape {
                        on_close.call(());
                    }
                },

                div { class: "pick-head",
                    div {
                        span { class: "pick-kicker", "Adding to" }
                        h2 { class: "pick-title", id: "add-books-title", "{shelf_name}" }
                    }
                    button {
                        r#type: "button",
                        class: "pick-close",
                        "aria-label": "Close",
                        "data-testid": "add-books-close",
                        onclick: move |_| on_close.call(()),
                        "\u{2715}"
                    }
                }

                LibraryPicker {
                    server_url: server_url.clone(),
                    picked,
                    already: members,
                    search_testid: "add-books-search",
                    autofocus: true,
                }

                if let Some(msg) = error() {
                    p {
                        role: "alert",
                        class: "shelf-modal-error",
                        "data-testid": "add-books-error",
                        "Couldn\u{2019}t add these books: {msg}"
                    }
                }

                div { class: "pick-foot",
                    button {
                        r#type: "button",
                        class: "btn shelf-btn-ghost",
                        "data-testid": "add-books-cancel",
                        onclick: move |_| on_close.call(()),
                        "Cancel"
                    }
                    button {
                        r#type: "button",
                        class: "btn shelf-btn-primary",
                        "data-testid": "add-books-submit",
                        disabled: saving() || count == 0,
                        onclick: on_add,
                        if saving() { "Adding\u{2026}" } else { "{add_label}" }
                    }
                }
            }
        }
    }
}
