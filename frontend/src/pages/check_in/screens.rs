//! One component per screen of the check-in flow, from the resolve spinner
//! onwards. The two input screens are big enough to own their files: `scan`
//! (camera) and `entry` (keypad).
//!
//! These are presentational: every write goes back out through an
//! [`EventHandler`] so [`super::CheckInPage`] owns the transport and the
//! [`super::Stage`] transitions. Visible text here is the Maestro selector
//! contract — keep it stable (rule 04, Mobile E2E).

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{ExternalBookMeta, ScanBook, WishlistAddRequest};

use super::{wishlist_request_for, FlowState};
use crate::{media_url, use_server_url, Route};

/// Matching spinner shown while the resolve request is in flight.
#[component]
pub(super) fn ResolvingScreen() -> Element {
    rsx! {
        div { class: "check-in-screen", "data-testid": "check-in-resolving",
            div { class: "check-in-spinner" }
            h1 { "Matching\u{2026}" }
            p { class: "subtitle", "Checking your library, then the web." }
        }
    }
}

/// 3a — confirm checking in a copy of a book the library already holds.
#[component]
pub(super) fn ConfirmScreen(
    book: ScanBook,
    isbn: String,
    state: FlowState,
    on_check_in: EventHandler<ScanBook>,
    on_cancel: EventHandler<()>,
) -> Element {
    let mut note = state.note;
    let busy = state.busy;
    let target = book.clone();
    rsx! {
        div { class: "check-in-screen", "data-testid": "check-in-confirm",
            h1 { "Check in this copy" }
            p { class: "subtitle", "You already have this one digitally \u{2014} this adds your print copy." }
            LibraryBookCard { book }
            div { class: "settings-field",
                label { r#for: "check-in-note", "Edition note (optional)" }
                input {
                    id: "check-in-note",
                    "data-testid": "check-in-note",
                    r#type: "text",
                    placeholder: "Paperback, 2019 reissue\u{2026}",
                    value: "{note}",
                    disabled: busy(),
                    oninput: move |e| note.set(e.value()),
                }
            }
            p { class: "check-in-isbn-line", "Scanned ISBN {isbn}" }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn primary",
                    disabled: busy(),
                    "data-testid": "check-in-confirm-submit",
                    onclick: move |_| on_check_in.call(target.clone()),
                    "Check in"
                }
                button {
                    r#type: "button",
                    class: "btn ghost",
                    disabled: busy(),
                    "data-testid": "check-in-cancel",
                    onclick: move |_| on_cancel.call(()),
                    "Cancel"
                }
            }
        }
    }
}

/// 2b — a fuzzy (title, author) hit. Never auto-resolved: the reader confirms
/// it, seeing both ISBNs, or falls through to the 3c chooser.
#[component]
pub(super) fn CloseMatchScreen(
    book: ScanBook,
    scanned: ExternalBookMeta,
    on_yes: EventHandler<()>,
    on_no: EventHandler<ExternalBookMeta>,
) -> Element {
    let library_isbn = book.isbn.clone();
    let fallthrough = scanned.clone();
    rsx! {
        div { class: "check-in-screen", "data-testid": "check-in-close-match",
            h1 { "Is this the book?" }
            p { class: "subtitle",
                "The ISBN you entered isn't on any book in your library, but the title and author match this one."
            }
            LibraryBookCard { book }
            p { class: "check-in-isbn-line", "Scanned ISBN {scanned.isbn13}" }
            if let Some(isbn) = library_isbn {
                p { class: "check-in-isbn-line", "Library edition ISBN {isbn}" }
            }
            p { class: "check-in-why",
                "Print and digital editions carry different ISBNs, so we ask before checking a copy in against the wrong book."
            }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn primary",
                    "data-testid": "check-in-close-match-yes",
                    onclick: move |_| on_yes.call(()),
                    "Yes, that's it"
                }
                button {
                    r#type: "button",
                    class: "btn ghost",
                    "data-testid": "check-in-close-match-no",
                    onclick: move |_| on_no.call(fallthrough.clone()),
                    "No, different book"
                }
            }
        }
    }
}

/// 3c — resolved online but absent from the library: own it, wishlist it, or
/// start over.
#[component]
pub(super) fn ChooseScreen(
    online: ExternalBookMeta,
    busy: Signal<bool>,
    on_own_it: EventHandler<ExternalBookMeta>,
    on_wishlist: EventHandler<WishlistAddRequest>,
    on_restart: EventHandler<()>,
) -> Element {
    let own_meta = online.clone();
    let wish_meta = online.clone();
    rsx! {
        div { class: "check-in-screen", "data-testid": "check-in-choose",
            h1 { "Not in your library" }
            p { class: "subtitle", "We found it online. What would you like to do?" }
            ExternalBookCard { meta: online }
            div { class: "check-in-actions",
                button {
                    r#type: "button",
                    class: "btn primary",
                    disabled: busy(),
                    "data-testid": "check-in-own-it",
                    onclick: move |_| on_own_it.call(own_meta.clone()),
                    "I own it \u{2014} add to my collection"
                }
                button {
                    r#type: "button",
                    class: "btn ghost",
                    disabled: busy(),
                    "data-testid": "check-in-wishlist",
                    onclick: move |_| on_wishlist.call(wishlist_request_for(&wish_meta)),
                    "Add to my wishlist"
                }
                button {
                    r#type: "button",
                    class: "btn ghost",
                    disabled: busy(),
                    "data-testid": "check-in-restart",
                    onclick: move |_| on_restart.call(()),
                    "Not the right book?"
                }
            }
        }
    }
}

/// Neither the library nor any provider knew the ISBN.
#[component]
pub(super) fn UnresolvedScreen(isbn: String, on_restart: EventHandler<()>) -> Element {
    rsx! {
        div { class: "check-in-screen", "data-testid": "check-in-unresolved",
            h1 { "We couldn't find that ISBN" }
            p { class: "subtitle",
                "Nothing in your library or our metadata providers matches {isbn}."
            }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn",
                    "data-testid": "check-in-restart",
                    onclick: move |_| on_restart.call(()),
                    "Try another ISBN"
                }
            }
        }
    }
}

/// 4 — the collector-delight landing for a completed check-in, reused (with a
/// different headline and no book link) for a wishlist add.
#[component]
pub(super) fn SuccessScreen(
    title: String,
    headline: String,
    book_uuid: Option<String>,
    on_restart: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "check-in-screen check-in-success", "data-testid": "check-in-success",
            div { class: "check-in-rings",
                span { class: "check-in-ring" }
                span { class: "check-in-ring" }
                span { class: "check-in-tick", "\u{2713}" }
            }
            h1 { "{headline}" }
            p { class: "subtitle", "{title}" }
            if book_uuid.is_none() {
                p { class: "check-in-why",
                    "Wishlisted books have no files \u{2014} they'll appear on your wishlist shelf until someone checks a copy in."
                }
            }
            div { class: "settings-actions",
                if let Some(uuid) = book_uuid {
                    Link {
                        to: Route::BookDetail { uuid },
                        class: "btn",
                        "data-testid": "check-in-view-book",
                        "View book"
                    }
                }
                button {
                    r#type: "button",
                    class: "btn ghost",
                    "data-testid": "check-in-scan-another",
                    onclick: move |_| on_restart.call(()),
                    "Check in another"
                }
            }
        }
    }
}

/// Cover + title + authors for a library book on a confirm screen.
#[component]
fn LibraryBookCard(book: ScanBook) -> Element {
    let server_url = use_server_url();
    let cover = book.cover_url.as_deref().map(|p| media_url(&server_url, p));
    rsx! {
        BookCard { cover, title: book.title.clone(), byline: byline(&book.authors) }
    }
}

/// Cover + title + byline for a provider-resolved book on the 3c chooser. The
/// cover URL is provider-hosted and used verbatim.
#[component]
fn ExternalBookCard(meta: ExternalBookMeta) -> Element {
    let byline = match (byline(&meta.authors), meta.year.as_deref()) {
        (a, Some(y)) if !a.is_empty() => format!("{a} \u{b7} {y}"),
        (a, _) if !a.is_empty() => a,
        (_, Some(y)) => y.to_string(),
        _ => String::new(),
    };
    rsx! {
        BookCard { cover: meta.cover_url.clone(), title: meta.title.clone(), byline }
    }
}

/// Shared card shell for both book flavors.
#[component]
fn BookCard(cover: Option<String>, title: String, byline: String) -> Element {
    // A provider cover can 404 or be blocked outright; fall back to the blank
    // plate rather than leaving the browser's broken-image glyph on screen.
    let mut broken = use_signal(|| false);
    rsx! {
        div { class: "check-in-book", "data-testid": "check-in-book",
            match cover {
                Some(src) if !broken() => rsx! {
                    img {
                        class: "check-in-book-cover",
                        src: "{src}",
                        alt: "",
                        onerror: move |_| broken.set(true),
                    }
                },
                _ => rsx! { div { class: "check-in-book-cover check-in-book-cover--empty" } },
            }
            div { class: "check-in-book-meta",
                strong { class: "check-in-book-title", "{title}" }
                if !byline.is_empty() {
                    span { class: "check-in-book-byline", "{byline}" }
                }
            }
        }
    }
}

/// Join contributor names for display: "A", "A and B", "A, B and C".
pub(super) fn byline(authors: &[String]) -> String {
    let names: Vec<&str> = authors
        .iter()
        .map(|a| a.trim())
        .filter(|a| !a.is_empty())
        .collect();
    match names.split_last() {
        None => String::new(),
        Some((last, [])) => (*last).to_string(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
    }
}
