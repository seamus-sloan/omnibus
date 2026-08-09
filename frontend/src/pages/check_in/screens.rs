//! One component per screen of the check-in flow, from the resolve spinner
//! onwards. The two input screens are big enough to own their files: `scan`
//! (camera) and `entry` (keypad).
//!
//! These are presentational: every write goes back out through an
//! [`EventHandler`] so [`super::CheckInPage`] owns the transport and the
//! [`super::Stage`] transitions. Visible text here is the E2E selector
//! contract — keep it stable (rule 04).

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

/// 2b — the fuzzy (title, author) hits. Never auto-resolved: the reader picks
/// the book they're holding, seeing both ISBNs, or falls through to the 3c
/// chooser.
///
/// Always a picker, because more than one row can carry the same effective
/// norm — an EPUB and the audiobook nothing ever attached to it are one work
/// in two rows, and declining that pair would offer to create a third.
#[component]
pub(super) fn CloseMatchScreen(
    books: Vec<ScanBook>,
    scanned: ExternalBookMeta,
    on_yes: EventHandler<ScanBook>,
    on_no: EventHandler<ExternalBookMeta>,
) -> Element {
    let many = books.len() > 1;
    let copy = CloseMatchCopy::for_count(many);
    let fallthrough = scanned.clone();
    rsx! {
        div { class: "check-in-screen", "data-testid": "check-in-close-match",
            h1 { "{copy.heading}" }
            p { class: "subtitle", "{copy.subtitle}" }
            p { class: "check-in-isbn-line", "Scanned ISBN {scanned.isbn13}" }
            ul { class: "check-in-match-list", "data-testid": "check-in-close-match-list",
                for book in books {
                    LibraryPickOption {
                        key: "{book.uuid}",
                        book,
                        pick_label: copy.pick.to_string(),
                        pick_testid: "check-in-close-match-pick".to_string(),
                        on_pick: on_yes,
                    }
                }
            }
            p { class: "check-in-why",
                "Print and digital editions carry different ISBNs, so we ask before checking a copy in against the wrong book."
            }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn ghost",
                    "data-testid": "check-in-close-match-no",
                    onclick: move |_| on_no.call(fallthrough.clone()),
                    "{copy.decline}"
                }
            }
        }
    }
}

/// The close-match screen's four strings, which all swap together on whether
/// the ladder offered one candidate or several.
pub(super) struct CloseMatchCopy {
    pub(super) heading: &'static str,
    pub(super) subtitle: &'static str,
    pub(super) pick: &'static str,
    pub(super) decline: &'static str,
}

impl CloseMatchCopy {
    /// `many` is `books.len() > 1`.
    pub(super) fn for_count(many: bool) -> Self {
        if many {
            Self {
                heading: "Which one is this?",
                subtitle: "The ISBN you entered isn't on any book in your library, but the title and author match these.",
                pick: "This one",
                decline: "None of these",
            }
        } else {
            Self {
                heading: "Is this the book?",
                subtitle: "The ISBN you entered isn't on any book in your library, but the title and author match this one.",
                pick: "Yes, that's it",
                decline: "No, different book",
            }
        }
    }
}

/// One row of a library-book picker: the library card, that edition's ISBN
/// when it has one, and the button that checks the scanned copy in against it.
///
/// Shared by the two screens that ask "which of these is the book you're
/// holding?" — the close-match candidates the ladder proposed, and the
/// [`super::link::LinkExistingScreen`] results the reader searched for
/// themselves. Same question, same `ScanBook`, same answer, so the label and
/// the testid are the only per-screen parts.
#[component]
pub(super) fn LibraryPickOption(
    book: ScanBook,
    pick_label: String,
    pick_testid: String,
    on_pick: EventHandler<ScanBook>,
) -> Element {
    let library_isbn = book.isbn.clone();
    let picked = book.clone();
    rsx! {
        li { class: "check-in-match-option",
            LibraryBookCard { book }
            if let Some(isbn) = library_isbn {
                p { class: "check-in-isbn-line", "Library edition ISBN {isbn}" }
            }
            button {
                r#type: "button",
                class: "btn primary",
                "data-testid": "{pick_testid}",
                onclick: move |_| on_pick.call(picked.clone()),
                "{pick_label}"
            }
        }
    }
}

/// 3c — resolved online but absent from the library: own it, link it to a
/// book already on the shelf, wishlist it, or start over.
#[component]
pub(super) fn ChooseScreen(
    online: ExternalBookMeta,
    state: FlowState,
    on_own_it: EventHandler<ExternalBookMeta>,
    on_wishlist: EventHandler<WishlistAddRequest>,
    on_link: EventHandler<()>,
    on_restart: EventHandler<()>,
) -> Element {
    let busy = state.busy;
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
                // A provider record can carry a placeholder title that shares
                // nothing with the library book, which no matching rung can
                // bridge — this is the reader's way to say so before a
                // duplicate exists to merge.
                button {
                    r#type: "button",
                    class: "btn ghost",
                    disabled: busy(),
                    "data-testid": "check-in-link-existing",
                    onclick: move |_| on_link.call(()),
                    "I already have this book"
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

/// Neither the library nor any provider knew the ISBN. Offers the title
/// search as the way forward — some editions simply aren't in any ISBN index.
#[component]
pub(super) fn UnresolvedScreen(
    isbn: String,
    on_search: EventHandler<()>,
    on_link: EventHandler<()>,
    on_restart: EventHandler<()>,
) -> Element {
    rsx! {
        div { class: "check-in-screen", "data-testid": "check-in-unresolved",
            h1 { "We couldn't find that ISBN" }
            p { class: "subtitle",
                "Nothing in your library or our metadata providers matches {isbn}."
            }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn primary",
                    "data-testid": "check-in-search-instead",
                    onclick: move |_| on_search.call(()),
                    "Search by title instead"
                }
                // No provider knows the barcode, but the reader may well own
                // the book already — filing the copy needs no provider at all.
                button {
                    r#type: "button",
                    class: "btn ghost",
                    "data-testid": "check-in-link-existing",
                    onclick: move |_| on_link.call(()),
                    "I already have this book"
                }
                button {
                    r#type: "button",
                    class: "btn ghost",
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

/// Cover + title + authors for a library book on a confirm screen, and for
/// each row of the link-to-an-existing-book picker.
#[component]
pub(super) fn LibraryBookCard(book: ScanBook) -> Element {
    let server_url = use_server_url();
    let cover = book.cover_url.as_deref().map(|p| media_url(&server_url, p));
    rsx! {
        BookCard { cover, title: book.title.clone(), byline: byline(&book.authors) }
    }
}

/// Cover + title + byline + provider facts for a resolved book — the 3c
/// chooser card, reused as a title-search result row. The cover URL is
/// provider-hosted and used verbatim.
#[component]
pub(super) fn ExternalBookCard(meta: ExternalBookMeta) -> Element {
    let byline = match (byline(&meta.authors), meta.year.as_deref()) {
        (a, Some(y)) if !a.is_empty() => format!("{a} \u{b7} {y}"),
        (a, _) if !a.is_empty() => a,
        (_, Some(y)) => y.to_string(),
        _ => String::new(),
    };
    rsx! {
        BookCard {
            cover: meta.cover_url.clone(),
            title: meta.title.clone(),
            byline,
            series: meta.series.clone(),
            detail: meta_details(&meta),
        }
    }
}

/// Shared card shell for both book flavors. `series` and `detail` are the
/// provider-fact lines only [`ExternalBookCard`] fills.
#[component]
fn BookCard(
    cover: Option<String>,
    title: String,
    byline: String,
    series: Option<String>,
    detail: Option<String>,
) -> Element {
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
                if let Some(series) = series {
                    span { class: "check-in-book-series", "data-testid": "check-in-book-series",
                        "{series}"
                    }
                }
                if !byline.is_empty() {
                    span { class: "check-in-book-byline", "{byline}" }
                }
                if let Some(detail) = detail {
                    span { class: "check-in-book-detail", "data-testid": "check-in-book-detail",
                        "{detail}"
                    }
                }
            }
        }
    }
}

/// The dot-separated fact line under a provider card's byline: first-publish
/// year, publisher, page count — whichever the provider carried. `None` when
/// it carried none of them, so the card renders no empty line.
pub(super) fn meta_details(meta: &ExternalBookMeta) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(year) = meta.first_publish_year {
        parts.push(format!("First published {year}"));
    }
    if let Some(publisher) = meta
        .publisher
        .as_deref()
        .map(str::trim)
        .filter(|p| !p.is_empty())
    {
        parts.push(publisher.to_string());
    }
    if let Some(pages) = meta.pages.filter(|p| *p > 0) {
        parts.push(format!("{pages} pages"));
    }
    (!parts.is_empty()).then(|| parts.join(" \u{b7} "))
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
