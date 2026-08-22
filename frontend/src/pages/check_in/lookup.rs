//! The lookup screen — the check-in flow's front door everywhere the camera
//! isn't the point. Both ways in sit on one screen: the ISBN off the back
//! cover, and a title search for editions no ISBN index carries. The scanner is
//! one explicit button away, so opening check-in never lights up a webcam.
//! Visible text here is the E2E selector contract — keep it stable.

use dioxus::prelude::*;
use omnibus_shared::ExternalBookMeta;

use super::search::TitleSearch;
use super::{clean_isbn, isbn_rejection, FlowState};
use crate::{data, use_server_url};

/// ISBN entry and title search together, with the camera as a secondary
/// action. `on_resolve` runs the ISBN ladder, `on_pick` takes a search
/// candidate, and `on_scan` hands the flow to the scanner.
#[component]
pub(super) fn LookupScreen(
    state: FlowState,
    on_resolve: EventHandler<String>,
    on_pick: EventHandler<ExternalBookMeta>,
    on_scan: EventHandler<()>,
) -> Element {
    let mut isbn = state.isbn;
    let busy = state.busy;
    // Gate the submit on the shared validator rather than on length alone, so
    // a transposed digit is caught here instead of a round-trip later.
    let ready = isbn_rejection(&clean_isbn(&isbn())).is_none();

    rsx! {
        div { class: "check-in-screen check-in-lookup", "data-testid": "check-in-lookup",
            h1 { "Check in a book" }
            p { class: "subtitle",
                "Enter the ISBN from the back cover, or search by title."
            }
            form {
                class: "settings-form",
                "data-testid": "check-in-form",
                onsubmit: move |evt| {
                    evt.prevent_default();
                    on_resolve.call(isbn());
                },
                div { class: "settings-field",
                    label { r#for: "check-in-isbn", "ISBN" }
                    input {
                        id: "check-in-isbn",
                        "data-testid": "check-in-isbn",
                        r#type: "text",
                        inputmode: "numeric",
                        autocomplete: "off",
                        placeholder: "9780000000000",
                        value: "{isbn}",
                        disabled: busy(),
                        oninput: move |e| isbn.set(e.value()),
                    }
                }
                div { class: "settings-actions",
                    button {
                        r#type: "submit",
                        class: "btn primary",
                        disabled: busy() || !ready,
                        "data-testid": "check-in-submit",
                        "Find book"
                    }
                }
            }
            p { class: "check-in-or", "data-testid": "check-in-or", "or" }
            TitleSearch { state, on_pick }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn ghost",
                    disabled: busy(),
                    "data-testid": "check-in-scan-instead",
                    onclick: move |_| on_scan.call(()),
                    "Scan a barcode"
                }
            }
            ProviderNote {}
        }
    }
}

/// The Open-Library-fallback disclaimer, shown under whichever input screen
/// is fronting the provider ladder — this one or the scanner.
///
/// `data::google_books_configured` is a non-admin-gated boolean check (any
/// authenticated user may read it), so every caller — web and mobile alike —
/// can fetch it directly with no role check and no guaranteed-403 round trip
/// for non-admins. Starts `None` on both SSR and the first WASM paint, so the
/// note only appears after the post-mount fetch resolves (rule 07).
#[component]
pub(super) fn ProviderNote() -> Element {
    let server_url = use_server_url();
    let mut key_configured = use_signal(|| None::<bool>);
    use_effect(move || {
        let url = server_url.clone();
        spawn(async move {
            if let Ok(configured) = data::google_books_configured(&url).await {
                key_configured.set(Some(configured));
            }
        });
    });

    rsx! {
        if key_configured() == Some(false) {
            p { class: "check-in-provider-note", "data-testid": "check-in-google-books-note",
                "Without a Google Books API key, this search will use Open Library, which may not have the latest books or all ISBNs. Set up your Google Books API key in Settings."
            }
        }
    }
}
