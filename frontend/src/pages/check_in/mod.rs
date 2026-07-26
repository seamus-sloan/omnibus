//! Check-in flow (`/check-in`) — resolve an ISBN and land it on the right
//! branch of the physical-ownership decision tree. The camera scanner is the
//! front door and manual entry its always-available fallback; both feed the
//! same [`Stage`] machine. Every screen is plain rsx with no target gating, so
//! SSR and the first WASM paint agree (rule 07).

mod entry;
mod scan;
mod screens;
#[cfg(test)]
mod tests;

use dioxus::prelude::*;
use dioxus_router::{use_navigator, use_route, Link};
use omnibus_shared::{
    isbn::normalize_isbn, AddPhysicalOnlyRequest, CheckInRequest, ExternalBookMeta, ResolveRequest,
    ScanBook, ScanOutcome, WishlistAddRequest, WishlistSource,
};

use crate::{data, use_server_url, Route};
use entry::EntryScreen;
use scan::ScanScreen;
use screens::{
    ChooseScreen, CloseMatchScreen, ConfirmScreen, ResolvingScreen, SuccessScreen, UnresolvedScreen,
};

/// Whether the check-in overlay is open. Provided at [`crate::App`] scope so
/// every entry point (top nav, add-books sheet, account row) can raise it and
/// the overlay — mounted once at the `ScreenLayout` root — can read it.
///
/// Starts closed, so SSR and the first WASM paint render no modal and agree
/// (rule 07).
#[derive(Copy, Clone, PartialEq)]
pub struct CheckInOpen(pub Signal<bool>);

/// Centered check-in overlay: the [`CheckInPage`] flow floating in a card over
/// a blurred scrim of the current page.
///
/// Mounted once at the `ScreenLayout` root (like the search palette) so its
/// `position: fixed` scrim covers the whole app and clicking outside closes it
/// — placed inside `TopNav`, the topbar's `backdrop-filter` would become the
/// containing block and shrink the scrim to the header strip. The `/check-in`
/// route still renders the same flow full-page as a deep-link fallback.
#[component]
pub fn CheckInOverlay() -> Element {
    let open = use_context::<CheckInOpen>().0;
    rsx! {
        if open() {
            CheckInModal {}
        }
    }
}

/// The scrim + card wrapper, mounted only while the overlay is open so the
/// flow's signals reset on every fresh open.
#[component]
fn CheckInModal() -> Element {
    let mut open = use_context::<CheckInOpen>().0;
    // The overlay lives in `ScreenLayout` and survives route changes, so a
    // check-in that navigates away (an already-owned scan, or "View book" on
    // the success screen) would otherwise strand the modal over the new page.
    // Reading the route subscribes this component; dismiss on any change from
    // the route we opened over. Comparing against the mount route (not a bare
    // "close on every run") keeps the effect's mount pass from closing us
    // immediately.
    let route = use_route::<Route>();
    let opened_over = use_hook(|| route.clone());
    use_effect(use_reactive!(|route| {
        if route != opened_over {
            open.set(false);
        }
    }));

    let mut close = move || open.set(false);
    rsx! {
        div {
            class: "check-in-overlay-scrim",
            "data-testid": "check-in-overlay-scrim",
            onclick: move |_| close(),
            div {
                class: "check-in-overlay-panel",
                "data-testid": "check-in-overlay-panel",
                role: "dialog",
                aria_modal: "true",
                aria_label: "Check in a book",
                tabindex: "-1",
                // Move focus into the dialog on open so keyboard users land
                // inside the modal and the panel's Escape handler fires without
                // a prior click. Mirrors the search palette's input focus.
                onmounted: move |evt: MountedEvent| focus_overlay_panel(&evt),
                // Clicks inside the card must not reach the scrim's close.
                onclick: move |e| e.stop_propagation(),
                onkeydown: move |e| {
                    if e.key() == Key::Escape {
                        close();
                    }
                },
                button {
                    class: "check-in-overlay-close",
                    r#type: "button",
                    aria_label: "Close",
                    "data-testid": "check-in-overlay-close",
                    onclick: move |_| close(),
                    "\u{00d7}"
                }
                CheckInPage {}
            }
        }
    }
}

/// Focus the dialog panel after the browser paints it, so the modal opens with
/// focus inside it and Escape works without a prior click. The
/// `requestAnimationFrame` hop lets layout finish first — a synchronous
/// `.focus()` inside `onmounted` lands before the element is laid out and
/// no-ops. Mirrors `focus_palette_input` in the search palette.
#[cfg(feature = "web")]
fn focus_overlay_panel(evt: &MountedEvent) {
    use dioxus::web::WebEventExt;
    use wasm_bindgen::prelude::*;

    let Some(element) = evt.try_as_web_event() else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    let cb = Closure::once_into_js(move || {
        if let Some(html_el) = element.dyn_ref::<web_sys::HtmlElement>() {
            let _ = html_el.focus();
        }
    });
    let _ = window.request_animation_frame(cb.unchecked_ref());
}

/// No-op on SSR / native: the panel only needs programmatic focus on the web
/// client, and mobile has no hardware Escape key.
#[cfg(not(feature = "web"))]
fn focus_overlay_panel(_evt: &MountedEvent) {}

/// Where the flow currently sits. One variant per leaf of the design's
/// decision tree, plus the three transient states (scan, entry, resolving).
#[derive(Clone, PartialEq)]
pub(crate) enum Stage {
    /// Camera scanner — the flow's start and its "scan another" target.
    Scan,
    /// Manual ISBN entry, reached from the scanner (asked for, or forced by a
    /// camera the device won't give us).
    Entry,
    /// Resolve request in flight.
    Resolving,
    /// 3a — confirm checking in a copy of a book we already hold.
    Confirm { book: ScanBook, isbn: String },
    /// 2b — a fuzzy (title, author) hit that needs a human "is this it?".
    CloseMatch {
        book: ScanBook,
        scanned: ExternalBookMeta,
    },
    /// 3c — resolved online but not in the library: own it, or wishlist it.
    Choose { online: ExternalBookMeta },
    /// Neither the library nor any provider knew the ISBN.
    Unresolved { isbn: String },
    /// 4 — the copy is checked in.
    CheckedIn { uuid: String, title: String },
    /// The book went on the caller's physical wishlist instead.
    Wishlisted { title: String },
}

/// Map a resolved outcome onto the screen it opens.
///
/// [`ScanOutcome::AlreadyOwned`] and [`ScanOutcome::OnWishlist`] have no stage
/// of their own: the design routes an already-owned or already-wishlisted book
/// straight to its detail page, so the caller navigates and this returns `None`.
pub(crate) fn stage_for(outcome: ScanOutcome, isbn: &str) -> Option<Stage> {
    match outcome {
        ScanOutcome::AlreadyOwned { .. } | ScanOutcome::OnWishlist { .. } => None,
        ScanOutcome::InLibraryUnowned { book } => Some(Stage::Confirm {
            book,
            isbn: isbn.to_string(),
        }),
        ScanOutcome::CloseMatch { book, scanned } => Some(Stage::CloseMatch { book, scanned }),
        ScanOutcome::NotInLibrary { online } => Some(Stage::Choose { online }),
        ScanOutcome::Unresolved => Some(Stage::Unresolved {
            isbn: isbn.to_string(),
        }),
    }
}

/// Strip an ISBN of the separators barcodes and back covers print with,
/// upper-casing the ISBN-10 `X` check digit. The server does the authoritative
/// validation; this only keeps typed input tidy.
pub(crate) fn clean_isbn(raw: &str) -> String {
    raw.chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// Validate `cleaned` as an ISBN-10 or ISBN-13, check digit included, so a
/// mistyped keypad entry or a misread barcode never costs a provider
/// round-trip. Returns the rejection sentence to show, or `None` when the
/// input is good. Shares [`normalize_isbn`] with the server, so the two agree
/// on what a valid ISBN is.
pub(crate) fn isbn_rejection(cleaned: &str) -> Option<String> {
    normalize_isbn(cleaned).err().map(|e| match e {
        // The length error names the count, which reads oddly next to a
        // keypad that shows the count already.
        omnibus_shared::isbn::IsbnError::InvalidLength(_) => {
            "Enter a 10- or 13-digit ISBN.".to_string()
        }
        other => other.to_string(),
    })
}

/// Strip the server-function plumbing a `ServerFnError` stringifies into, so
/// the screen shows the server's own sentence ("ISBN has an invalid check
/// digit") instead of the transport's framing.
pub(crate) fn friendly_error(raw: &str) -> String {
    let trimmed = raw
        .trim_start_matches("error running server function:")
        .trim_end_matches("(details: None)")
        .trim();
    if trimmed.is_empty() {
        raw.trim().to_string()
    } else {
        trimmed.to_string()
    }
}

/// The flow's shared signals, `Copy` so every async handler can capture them.
/// `isbn` lives here rather than in the entry screen so a failed lookup leaves
/// the typed value in place to correct.
#[derive(Copy, Clone, PartialEq)]
pub(crate) struct FlowState {
    pub(crate) stage: Signal<Stage>,
    pub(crate) isbn: Signal<String>,
    pub(crate) note: Signal<String>,
    pub(crate) busy: Signal<bool>,
    pub(crate) error: Signal<Option<String>>,
}

/// Check-in page: type an ISBN, then follow it wherever the ladder lands.
#[component]
pub fn CheckInPage() -> Element {
    let server_url = use_server_url();
    let nav = use_navigator();
    let state = FlowState {
        stage: use_signal(|| Stage::Scan),
        isbn: use_signal(String::new),
        note: use_signal(String::new),
        busy: use_signal(|| false),
        error: use_signal(|| None),
    };

    let on_resolve = make_on_resolve(server_url.clone(), state, nav);
    let on_check_in = make_on_check_in(server_url.clone(), state);
    let on_own_it = make_on_own_it(server_url.clone(), state);
    let on_wishlist = make_on_wishlist(server_url, state);

    rsx! {
        section { class: "card check-in", "data-testid": "check-in",
            // Present on every stage so a mid-flow abort never strands the reader.
            Link {
                to: Route::Landing {},
                class: "check-in-close",
                "data-testid": "check-in-close",
                "aria-label": "Cancel check-in",
                title: "Cancel",
                "\u{00d7}"
            }
            CheckInStage {
                state,
                on_resolve: EventHandler::new(on_resolve),
                on_check_in: EventHandler::new(on_check_in),
                on_own_it: EventHandler::new(on_own_it),
                on_wishlist: EventHandler::new(on_wishlist),
            }
            if let Some(msg) = (state.error)() {
                p {
                    "data-testid": "check-in-error",
                    role: "alert",
                    class: "settings-status error",
                    "{msg}"
                }
            }
        }
    }
}

/// Render whichever screen the current [`Stage`] calls for. Split out of
/// [`CheckInPage`] so the page body stays a thin handler-wiring shell.
#[component]
fn CheckInStage(
    state: FlowState,
    on_resolve: EventHandler<String>,
    on_check_in: EventHandler<ScanBook>,
    on_own_it: EventHandler<ExternalBookMeta>,
    on_wishlist: EventHandler<WishlistAddRequest>,
) -> Element {
    let mut stage = state.stage;
    let mut isbn = state.isbn;
    let mut error = state.error;
    let mut note = state.note;
    let mut busy = state.busy;
    // Clear the per-flow scratch (edition note, typed ISBN, in-flight flag) on
    // every restart so neither a note nor an ISBN typed for one book can be
    // reused on the next confirm after Cancel / "Try another ISBN" /
    // "Check in another".
    let restart = move |_| {
        error.set(None);
        note.set(String::new());
        isbn.set(String::new());
        busy.set(false);
        stage.set(Stage::Scan);
    };

    match stage() {
        Stage::Scan => rsx! {
            ScanScreen {
                on_detect: EventHandler::new(move |scanned: String| {
                    // Seed the keypad too, so a decode the check digit rejects
                    // lands on manual entry with the digits ready to fix.
                    isbn.set(scanned.clone());
                    on_resolve.call(scanned);
                }),
                on_manual: EventHandler::new(move |_| {
                    error.set(None);
                    stage.set(Stage::Entry);
                }),
            }
        },
        Stage::Entry => rsx! {
            EntryScreen {
                isbn: state.isbn,
                busy: state.busy,
                on_resolve,
                on_scan: EventHandler::new(move |_| {
                    error.set(None);
                    stage.set(Stage::Scan);
                }),
            }
        },
        Stage::Resolving => rsx! { ResolvingScreen {} },
        Stage::Confirm { book, isbn } => rsx! {
            ConfirmScreen { book, isbn, state, on_check_in, on_cancel: EventHandler::new(restart) }
        },
        Stage::CloseMatch { book, scanned } => rsx! {
            CloseMatchScreen {
                book: book.clone(),
                scanned: scanned.clone(),
                on_yes: EventHandler::new(move |_| {
                    stage.set(Stage::Confirm { book: book.clone(), isbn: scanned.isbn13.clone() });
                }),
                on_no: EventHandler::new(move |online: ExternalBookMeta| {
                    stage.set(Stage::Choose { online });
                }),
            }
        },
        Stage::Choose { online } => rsx! {
            ChooseScreen {
                online,
                busy: state.busy,
                on_own_it,
                on_wishlist,
                on_restart: EventHandler::new(restart),
            }
        },
        Stage::Unresolved { isbn } => rsx! {
            UnresolvedScreen { isbn, on_restart: EventHandler::new(restart) }
        },
        Stage::CheckedIn { uuid, title } => rsx! {
            SuccessScreen {
                title,
                headline: "In your physical collection".to_string(),
                book_uuid: Some(uuid),
                on_restart: EventHandler::new(restart),
            }
        },
        Stage::Wishlisted { title } => rsx! {
            SuccessScreen {
                title,
                headline: "On your wishlist".to_string(),
                book_uuid: None,
                on_restart: EventHandler::new(restart),
            }
        },
    }
}

/// Build the resolve handler: validate the typed ISBN, run the ladder, then
/// either navigate (already owned) or open the matching screen.
fn make_on_resolve(
    server_url: String,
    state: FlowState,
    nav: dioxus_router::Navigator,
) -> impl FnMut(String) {
    let FlowState {
        mut stage,
        mut busy,
        mut error,
        ..
    } = state;
    move |raw: String| {
        let isbn = clean_isbn(&raw);
        if let Some(msg) = isbn_rejection(&isbn) {
            error.set(Some(msg));
            // A bad decode must not strand the reader on a blank screen: drop
            // back to the keypad with the digits in place to correct.
            stage.set(Stage::Entry);
            return;
        }
        let server_url = server_url.clone();
        error.set(None);
        busy.set(true);
        stage.set(Stage::Resolving);
        spawn(async move {
            let req = ResolveRequest { isbn: isbn.clone() };
            match data::resolve_scan(&server_url, req).await {
                Ok(ScanOutcome::AlreadyOwned { book }) | Ok(ScanOutcome::OnWishlist { book }) => {
                    nav.push(Route::BookDetail { uuid: book.uuid });
                }
                Ok(outcome) => {
                    if let Some(next) = stage_for(outcome, &isbn) {
                        stage.set(next);
                    }
                }
                Err(e) => {
                    error.set(Some(format!(
                        "Could not look that up: {}",
                        friendly_error(&e.to_string())
                    )));
                    stage.set(Stage::Entry);
                }
            }
            busy.set(false);
        });
    }
}

/// Build the 3a check-in handler: file a physical copy against a library book.
fn make_on_check_in(server_url: String, state: FlowState) -> impl FnMut(ScanBook) {
    let FlowState {
        mut stage,
        note,
        mut busy,
        mut error,
        ..
    } = state;
    move |book: ScanBook| {
        let server_url = server_url.clone();
        let isbn = match stage() {
            Stage::Confirm { isbn, .. } => Some(isbn),
            _ => None,
        };
        let note = some_if_filled(&note());
        let title = book.title.clone();
        error.set(None);
        busy.set(true);
        spawn(async move {
            let req = CheckInRequest {
                book_uuid: book.uuid.clone(),
                isbn,
                note,
            };
            match data::check_in(&server_url, req).await {
                Ok(book_ref) => stage.set(Stage::CheckedIn {
                    uuid: book_ref.book_uuid,
                    title,
                }),
                Err(e) => error.set(Some(format!(
                    "Check-in failed: {}",
                    friendly_error(&e.to_string())
                ))),
            }
            busy.set(false);
        });
    }
}

/// Build the 3c "I own it" handler: create the fileless book + its first copy.
fn make_on_own_it(server_url: String, state: FlowState) -> impl FnMut(ExternalBookMeta) {
    let FlowState {
        mut stage,
        note,
        mut busy,
        mut error,
        ..
    } = state;
    move |meta: ExternalBookMeta| {
        let server_url = server_url.clone();
        let note = some_if_filled(&note());
        let title = meta.title.clone();
        error.set(None);
        busy.set(true);
        spawn(async move {
            let req = AddPhysicalOnlyRequest { meta, note };
            match data::add_physical_only(&server_url, req).await {
                Ok(book_ref) => stage.set(Stage::CheckedIn {
                    uuid: book_ref.book_uuid,
                    title,
                }),
                Err(e) => error.set(Some(format!(
                    "Could not add that book: {}",
                    friendly_error(&e.to_string())
                ))),
            }
            busy.set(false);
        });
    }
}

/// Build the wishlist handler. The request is assembled by the screen (it
/// knows whether it holds a library book or online metadata).
fn make_on_wishlist(server_url: String, state: FlowState) -> impl FnMut(WishlistAddRequest) {
    let FlowState {
        mut stage,
        mut busy,
        mut error,
        ..
    } = state;
    move |req: WishlistAddRequest| {
        let server_url = server_url.clone();
        let title = req
            .meta
            .as_ref()
            .map(|m| m.title.clone())
            .unwrap_or_default();
        error.set(None);
        busy.set(true);
        spawn(async move {
            match data::wishlist_add(&server_url, req).await {
                Ok(_) => stage.set(Stage::Wishlisted { title }),
                Err(e) => error.set(Some(format!(
                    "Could not add to your wishlist: {}",
                    friendly_error(&e.to_string())
                ))),
            }
            busy.set(false);
        });
    }
}

/// Trim `s`, keeping it only when something is left — the wire shape for the
/// optional edition note.
pub(crate) fn some_if_filled(s: &str) -> Option<String> {
    let trimmed = s.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The wishlist request for an online-resolved book on the 3c chooser.
pub(crate) fn wishlist_request_for(online: &ExternalBookMeta) -> WishlistAddRequest {
    WishlistAddRequest {
        book_uuid: None,
        meta: Some(online.clone()),
        source: WishlistSource::Scan,
    }
}
