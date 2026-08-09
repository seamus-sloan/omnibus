//! Check-in flow (`/check-in`) — resolve an ISBN and land it on the right
//! branch of the physical-ownership decision tree. The lookup screen (ISBN
//! and title search together) is the front door, with the camera scanner
//! behind an explicit button; the mobile shell inverts that. All of them feed
//! the same [`Stage`] machine. Every screen is plain rsx with no target
//! gating, so SSR and the first WASM paint agree (rule 07).

mod entry;
mod link;
mod lookup;
mod scan;
mod screens;
mod search;
#[cfg(test)]
mod tests;

use dioxus::prelude::*;
use dioxus_router::{use_navigator, use_route, Link};
use omnibus_shared::{
    isbn::normalize_isbn, AddPhysicalOnlyRequest, CheckInRequest, ExternalBookMeta,
    ResolveMetaRequest, ResolveRequest, ScanBook, ScanOutcome, WishlistAddRequest, WishlistSource,
};

use crate::focus_after_paint::focus_after_paint;
use crate::{data, use_server_url, Route};
use entry::EntryScreen;
use link::LinkExistingScreen;
use lookup::LookupScreen;
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
                onmounted: move |evt: MountedEvent| focus_after_paint(&evt),
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

/// Where the flow currently sits. One variant per leaf of the design's
/// decision tree, plus the three transient states (scan, entry, resolving).
#[derive(Clone, PartialEq)]
pub(crate) enum Stage {
    /// ISBN entry and title search on one screen — the front door off the
    /// mobile shell, and what every "search by title" action opens.
    Lookup,
    /// Camera scanner — the mobile shell's front door, and the lookup screen's
    /// "Scan a barcode" target everywhere else.
    Scan,
    /// Manual ISBN keypad, reached from the scanner (asked for, or forced by a
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
    /// The reader says the copy belongs to a book already on the shelf and is
    /// picking which one. Carries the ISBN to file the copy under, since the
    /// picked book won't be the one the ladder failed to reach, and the screen
    /// that opened it so Back restores that screen's resolved book rather than
    /// dropping the reader on a blank ISBN field.
    LinkExisting { isbn: String, origin: Box<Stage> },
    /// Neither the library nor any provider knew the ISBN.
    Unresolved { isbn: String },
    /// 4 — the copy is checked in.
    CheckedIn { uuid: String, title: String },
    /// The book went on the caller's physical wishlist instead.
    Wishlisted { title: String },
}

/// The stage the flow opens on, and the one every "start over" returns to.
///
/// The mobile shell leads with the camera — that's the whole point of a phone
/// in front of a bookshelf. Everywhere else leads with the fields, so opening
/// check-in never starts a camera nobody asked for. Gating the *value* rather
/// than any rsx keeps SSR and the first WASM paint identical (rule 07); the
/// mobile build renders client-side only and has no SSR markup to hydrate.
#[cfg(feature = "mobile")]
pub(crate) fn front_door() -> Stage {
    Stage::Scan
}

/// Web and SSR front door — see the `mobile` twin above.
#[cfg(not(feature = "mobile"))]
pub(crate) fn front_door() -> Stage {
    Stage::Lookup
}

/// Where a rejected or failed lookup drops the reader: back onto the screen
/// they typed on, so the digits are still in front of them to correct. A
/// camera decode has no such screen, so it lands on the keypad instead.
pub(crate) fn manual_fallback(origin: &Stage) -> Stage {
    match origin {
        Stage::Lookup => Stage::Lookup,
        _ => Stage::Entry,
    }
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

/// [`CheckInStage`]'s per-outcome callbacks, bundled into one prop so the
/// component stays under the 5-prop soft cap. One field per terminal action
/// a stage can trigger; [`CheckInPage`] wires each to its `make_on_*` handler.
#[derive(Clone, PartialEq)]
struct CheckInHandlers {
    on_resolve: EventHandler<String>,
    on_check_in: EventHandler<ScanBook>,
    on_own_it: EventHandler<ExternalBookMeta>,
    on_pick: EventHandler<ExternalBookMeta>,
    on_wishlist: EventHandler<WishlistAddRequest>,
}

/// Check-in page: type an ISBN, then follow it wherever the ladder lands.
#[component]
pub fn CheckInPage() -> Element {
    let server_url = use_server_url();
    let nav = use_navigator();
    let state = FlowState {
        stage: use_signal(front_door),
        isbn: use_signal(String::new),
        note: use_signal(String::new),
        busy: use_signal(|| false),
        error: use_signal(|| None),
    };

    let on_resolve = make_on_resolve(server_url.clone(), state, nav);
    let on_check_in = make_on_check_in(server_url.clone(), state);
    let on_own_it = make_on_own_it(server_url.clone(), state);
    let on_pick = make_on_pick(server_url.clone(), state, nav);
    let on_wishlist = make_on_wishlist(server_url, state);
    let handlers = CheckInHandlers {
        on_resolve: EventHandler::new(on_resolve),
        on_check_in: EventHandler::new(on_check_in),
        on_own_it: EventHandler::new(on_own_it),
        on_pick: EventHandler::new(on_pick),
        on_wishlist: EventHandler::new(on_wishlist),
    };

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
            CheckInStage { state, handlers }
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
/// [`CheckInPage`] so the page body stays a thin handler-wiring shell, and
/// each verbose arm is further split into a named `_stage` helper so this
/// dispatcher stays under the line cap.
#[component]
fn CheckInStage(state: FlowState, handlers: CheckInHandlers) -> Element {
    // Clear the per-flow scratch (edition note, typed ISBN, in-flight flag) on
    // every restart so neither a note nor an ISBN typed for one book can be
    // reused on the next confirm after Cancel / "Try another ISBN" /
    // "Check in another".
    let on_restart = EventHandler::new(move |_| {
        let FlowState {
            mut stage,
            mut isbn,
            mut note,
            mut busy,
            mut error,
        } = state;
        error.set(None);
        note.set(String::new());
        isbn.set(String::new());
        busy.set(false);
        stage.set(front_door());
    });

    match (state.stage)() {
        Stage::Lookup => lookup_stage(state, handlers.on_resolve, handlers.on_pick),
        Stage::Scan => scan_stage(state, handlers.on_resolve),
        Stage::Entry => entry_stage(state, handlers.on_resolve),
        Stage::Resolving => rsx! { ResolvingScreen {} },
        Stage::Confirm { book, isbn } => rsx! {
            ConfirmScreen { book, isbn, state, on_check_in: handlers.on_check_in, on_cancel: on_restart }
        },
        Stage::CloseMatch { book, scanned } => close_match_stage(book, scanned, state),
        Stage::Choose { online } => choose_stage(online, state, handlers, on_restart),
        Stage::LinkExisting { isbn, origin } => link_stage(isbn, *origin, state),
        Stage::Unresolved { isbn } => unresolved_stage(isbn, state, on_restart),
        Stage::CheckedIn { uuid, title } => rsx! {
            SuccessScreen {
                title,
                headline: "In your physical collection".to_string(),
                book_uuid: Some(uuid),
                on_restart,
            }
        },
        Stage::Wishlisted { title } => rsx! {
            SuccessScreen {
                title,
                headline: "On your wishlist".to_string(),
                book_uuid: None,
                on_restart,
            }
        },
    }
}

/// [`Stage::Lookup`]: the fields-first front door — ISBN entry and title
/// search together, with the camera behind an explicit button.
fn lookup_stage(
    state: FlowState,
    on_resolve: EventHandler<String>,
    on_pick: EventHandler<ExternalBookMeta>,
) -> Element {
    let mut error = state.error;
    let mut stage = state.stage;
    rsx! {
        LookupScreen {
            state,
            on_resolve,
            on_pick,
            on_scan: EventHandler::new(move |_| {
                error.set(None);
                stage.set(Stage::Scan);
            }),
        }
    }
}

/// [`Stage::Scan`]: the camera screen, seeding the keypad on every decode so a
/// scan the check digit rejects lands on manual entry ready to fix.
fn scan_stage(state: FlowState, on_resolve: EventHandler<String>) -> Element {
    let mut isbn = state.isbn;
    let mut error = state.error;
    let mut stage = state.stage;
    rsx! {
        ScanScreen {
            on_detect: EventHandler::new(move |scanned: String| {
                isbn.set(scanned.clone());
                on_resolve.call(scanned);
            }),
            on_manual: EventHandler::new(move |_| {
                error.set(None);
                stage.set(Stage::Entry);
            }),
            on_search: go_to_search(state),
        }
    }
}

/// [`Stage::Entry`]: the manual-ISBN keypad, reached from the scanner.
fn entry_stage(state: FlowState, on_resolve: EventHandler<String>) -> Element {
    let mut error = state.error;
    let mut stage = state.stage;
    rsx! {
        EntryScreen {
            isbn: state.isbn,
            busy: state.busy,
            on_resolve,
            on_scan: EventHandler::new(move |_| {
                error.set(None);
                stage.set(Stage::Scan);
            }),
            on_search: go_to_search(state),
        }
    }
}

/// Open [`Stage::Lookup`], where the title-search field lives, clearing any
/// error so a failed lookup's message can't follow the reader onto a screen it
/// no longer describes.
///
/// Shared by the scanner, the keypad, and the unresolved screen. On web the
/// lookup screen is already the front door, so this matters most on the mobile
/// shell — the camera starts there, and this is its one-tap route to a title
/// search.
pub(crate) fn go_to_search(state: FlowState) -> EventHandler<()> {
    let FlowState {
        mut stage,
        mut error,
        ..
    } = state;
    EventHandler::new(move |_| {
        error.set(None);
        stage.set(Stage::Lookup);
    })
}

/// [`Stage::CloseMatch`]: the fuzzy (title, author) hit that needs a human
/// "is this it?" before filing a copy or falling back to the online chooser.
fn close_match_stage(book: ScanBook, scanned: ExternalBookMeta, state: FlowState) -> Element {
    let mut stage = state.stage;
    rsx! {
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
    }
}

/// [`Stage::Choose`]: resolved online but absent from the library.
fn choose_stage(
    online: ExternalBookMeta,
    state: FlowState,
    handlers: CheckInHandlers,
    on_restart: EventHandler<()>,
) -> Element {
    let origin = Stage::Choose {
        online: online.clone(),
    };
    let isbn = online.isbn13.clone();
    rsx! {
        ChooseScreen {
            online,
            state,
            on_own_it: handlers.on_own_it,
            on_wishlist: handlers.on_wishlist,
            on_link: go_to_link(state, isbn, origin),
            on_restart,
        }
    }
}

/// [`Stage::LinkExisting`]: pick the library book this copy belongs to, then
/// hand it to the same confirm screen an exact-ISBN hit would have opened.
fn link_stage(isbn: String, origin: Stage, state: FlowState) -> Element {
    let mut stage = state.stage;
    let back_to = origin.clone();
    rsx! {
        LinkExistingScreen {
            state,
            on_pick: EventHandler::new(move |book: ScanBook| {
                stage.set(Stage::Confirm { book, isbn: isbn.clone() });
            }),
            on_back: EventHandler::new(move |_| stage.set(back_to.clone())),
        }
    }
}

/// Open [`Stage::LinkExisting`] for `isbn`, remembering `origin` so Back
/// restores the outcome screen the reader left rather than restarting.
fn go_to_link(state: FlowState, isbn: String, origin: Stage) -> EventHandler<()> {
    let FlowState {
        mut stage,
        mut error,
        ..
    } = state;
    EventHandler::new(move |_| {
        error.set(None);
        stage.set(Stage::LinkExisting {
            isbn: isbn.clone(),
            origin: Box::new(origin.clone()),
        });
    })
}

/// [`Stage::Unresolved`]: neither the library nor any provider knew the ISBN.
fn unresolved_stage(isbn: String, state: FlowState, on_restart: EventHandler<()>) -> Element {
    let origin = Stage::Unresolved { isbn: isbn.clone() };
    let link_isbn = isbn.clone();
    rsx! {
        UnresolvedScreen {
            isbn,
            on_search: go_to_search(state),
            on_link: go_to_link(state, link_isbn, origin),
            on_restart,
        }
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
        // Captured before the stage moves to `Resolving`, so the async error
        // path below still knows which screen the reader came from.
        let fallback = manual_fallback(&stage());
        let isbn = clean_isbn(&raw);
        if let Some(msg) = isbn_rejection(&isbn) {
            error.set(Some(msg));
            // A bad decode must not strand the reader on a blank screen: drop
            // back to a typing screen with the digits in place to correct.
            stage.set(fallback);
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
                    stage.set(fallback);
                }
            }
            busy.set(false);
        });
    }
}

/// Build the search-pick handler: resolve a picked candidate against the
/// library (no provider re-lookup, which could miss on a book the search just
/// surfaced), then navigate or open the matching screen like a scan would.
fn make_on_pick(
    server_url: String,
    state: FlowState,
    nav: dioxus_router::Navigator,
) -> impl FnMut(ExternalBookMeta) {
    let FlowState {
        mut stage,
        mut busy,
        mut error,
        ..
    } = state;
    move |meta: ExternalBookMeta| {
        let server_url = server_url.clone();
        let isbn = meta.isbn13.clone();
        error.set(None);
        busy.set(true);
        spawn(async move {
            let req = ResolveMetaRequest { meta };
            match data::resolve_scan_meta(&server_url, req).await {
                Ok(ScanOutcome::AlreadyOwned { book }) | Ok(ScanOutcome::OnWishlist { book }) => {
                    nav.push(Route::BookDetail { uuid: book.uuid });
                }
                Ok(outcome) => {
                    if let Some(next) = stage_for(outcome, &isbn) {
                        stage.set(next);
                    }
                }
                // Stay on the search screen with the results intact to retry.
                Err(e) => {
                    error.set(Some(format!(
                        "Could not match that book: {}",
                        friendly_error(&e.to_string())
                    )));
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
