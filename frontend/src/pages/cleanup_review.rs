//! Library-cleanup review page (`/settings/cleanup/:kind`) — one suggestion
//! card at a time, decided with Accept / Reject / Skip. Web/SSR only: the real
//! authorization boundary is the `AdminUser` extractor on the `cleanup/*`
//! server functions; the in-page `use_is_admin` gate keeps the chrome off a
//! non-admin screen.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::{CleanupAction, CleanupKind, Decision, SuggestionCard};

use crate::data::{self, CLEANUP_QUEUE_PAGE};
use crate::{use_is_admin, use_server_url};

/// Signals the queue reducer owns, threaded together so the handlers below
/// take one argument instead of five.
#[derive(Clone, Copy, PartialEq)]
struct QueueState {
    /// `None` until the first fetch resolves — the loading state.
    cards: Signal<Option<Vec<SuggestionCard>>>,
    /// Index of the card on screen. Past the end means the queue is worked
    /// through and the empty state shows.
    cursor: Signal<usize>,
    error: Signal<Option<String>>,
    in_flight: Signal<bool>,
}

/// The `/settings/cleanup/:kind` review surface. An unrecognized `kind` slug
/// renders the not-found notice rather than silently reviewing another kind.
#[component]
pub fn CleanupReviewPage(kind: String) -> Element {
    let is_admin = use_is_admin();
    let server_url = use_server_url();
    let parsed = CleanupKind::from_str(&kind);
    let state = QueueState {
        cards: use_signal(|| None),
        cursor: use_signal(|| 0usize),
        error: use_signal(|| None),
        in_flight: use_signal(|| false),
    };

    spawn_queue_fetch(is_admin, server_url.clone(), parsed, state);

    rsx! {
        section {
            class: "card cleanup-review",
            "data-testid": "cleanup-review",
            tabindex: "0",
            autofocus: true,
            onkeydown: move |evt: KeyboardEvent| {
                on_review_key(evt, server_url.clone(), state);
            },
            h1 { "{kind_title(parsed)}" }
            p { class: "subtitle",
                "Accept (Y), reject (N), or skip (Space) each suggestion."
            }
            if is_admin() {
                CleanupReviewBody { kind: parsed, state }
            } else {
                p { class: "settings-status error", "data-testid": "cleanup-review-forbidden",
                    "Administrator access is required to review library cleanup."
                }
            }
        }
    }
}

/// Load the pending queue once the visitor is known to be an admin. An unknown
/// kind slug resolves to an empty queue so the page still renders its not-found
/// notice rather than hanging on the loading state.
fn spawn_queue_fetch(
    is_admin: ReadSignal<bool>,
    server_url: String,
    kind: Option<CleanupKind>,
    state: QueueState,
) {
    let mut cards = state.cards;
    let mut cursor = state.cursor;
    let mut error = state.error;
    use_effect(move || {
        // Read inside the effect so it re-subscribes and re-runs when
        // `CurrentUser` resolves. It is `false` on the first paint — firing
        // then is a guaranteed 401/403 against the admin-gated route, and the
        // non-admin branch never renders the body these signals feed.
        if !is_admin() {
            return;
        }
        let server_url = server_url.clone();
        error.set(None);
        cursor.set(0);
        spawn(async move {
            let Some(kind) = kind else {
                cards.set(Some(Vec::new()));
                return;
            };
            match data::get_cleanup_queue(&server_url, kind, CLEANUP_QUEUE_PAGE).await {
                Ok(queue) => cards.set(Some(queue)),
                Err(e) => {
                    cards.set(Some(Vec::new()));
                    error.set(Some(crate::data::server_error_message(&e)));
                }
            }
        });
    });
}

/// Route a review hotkey to its action: `y` accepts, `n` rejects, Space skips.
/// Every other key falls through so the page keeps normal keyboard behaviour.
fn on_review_key(evt: KeyboardEvent, server_url: String, state: QueueState) {
    match review_key_action(&evt.key()) {
        Some(ReviewKey::Accept) => decide(server_url, state, Decision::Accepted),
        Some(ReviewKey::Reject) => decide(server_url, state, Decision::Rejected),
        Some(ReviewKey::Skip) => {
            evt.prevent_default();
            advance(state);
        }
        None => {}
    }
}

/// What a keypress means on the review surface.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReviewKey {
    Accept,
    Reject,
    Skip,
}

/// Map a `KeyboardEvent` key to its review action. Case-insensitive so
/// shift-held `Y` works, and deliberately narrow — anything else is not a
/// review hotkey.
fn review_key_action(key: &Key) -> Option<ReviewKey> {
    let Key::Character(c) = key else {
        return None;
    };
    match c.as_str() {
        "y" | "Y" => Some(ReviewKey::Accept),
        "n" | "N" => Some(ReviewKey::Reject),
        " " => Some(ReviewKey::Skip),
        _ => None,
    }
}

/// Move to the next card without deciding — Skip leaves the suggestion pending
/// so it comes back on the next pass.
fn advance(state: QueueState) {
    let mut cursor = state.cursor;
    cursor.set(cursor() + 1);
}

/// Send one Accept / Reject through `cleanup/decide`, then advance. Guarded on
/// `in_flight` so a held hotkey can't double-decide the same card.
fn decide(server_url: String, state: QueueState, decision: Decision) {
    let QueueState {
        cards,
        cursor,
        mut error,
        mut in_flight,
    } = state;
    if in_flight() {
        return;
    }
    let Some(card) = current_card(&cards.read(), cursor()) else {
        return;
    };
    let id = card.id;
    in_flight.set(true);
    error.set(None);
    spawn(async move {
        match data::decide_cleanup_suggestion(&server_url, id, decision).await {
            Ok(()) => advance(state),
            Err(e) => error.set(Some(crate::data::server_error_message(&e))),
        }
        in_flight.set(false);
    });
}

/// The card at `cursor`, or `None` once the queue is worked through.
fn current_card(cards: &Option<Vec<SuggestionCard>>, cursor: usize) -> Option<SuggestionCard> {
    cards.as_ref()?.get(cursor).cloned()
}

/// Heading for the kind under review.
fn kind_title(kind: Option<CleanupKind>) -> &'static str {
    match kind {
        Some(CleanupKind::Author) => "Review authors",
        Some(CleanupKind::Series) => "Review series",
        Some(CleanupKind::Tag) => "Review tags",
        Some(CleanupKind::BookTitle) => "Review book titles",
        None => "Review library cleanup",
    }
}

/// Loading / error / empty / card states for the review queue.
#[component]
fn CleanupReviewBody(kind: Option<CleanupKind>, state: QueueState) -> Element {
    let server_url = use_server_url();
    let error = (state.error)();
    let cards = (state.cards)();
    let card = current_card(&cards, (state.cursor)());
    rsx! {
        if kind.is_none() {
            p { class: "settings-status error", role: "status", "data-testid": "cleanup-review-unknown-kind",
                "Unknown cleanup kind."
            }
        }
        if let Some(message) = error {
            p { class: "settings-status error", role: "status", "data-testid": "cleanup-review-error",
                "{message}"
            }
        }
        if cards.is_none() {
            p { class: "settings-status", role: "status", "data-testid": "cleanup-review-loading",
                "Loading\u{2026}"
            }
        } else if let Some(card) = card {
            SuggestionCardView { card }
            div { class: "cleanup-review-actions",
                button {
                    class: "btn primary",
                    "data-testid": "cleanup-accept",
                    disabled: (state.in_flight)(),
                    onclick: {
                        let server_url = server_url.clone();
                        move |_| decide(server_url.clone(), state, Decision::Accepted)
                    },
                    "Accept (Y)"
                }
                button {
                    class: "btn",
                    "data-testid": "cleanup-reject",
                    disabled: (state.in_flight)(),
                    onclick: {
                        let server_url = server_url.clone();
                        move |_| decide(server_url.clone(), state, Decision::Rejected)
                    },
                    "Reject (N)"
                }
                button {
                    class: "btn",
                    "data-testid": "cleanup-skip",
                    onclick: move |_| advance(state),
                    "Skip (Space)"
                }
            }
        } else if kind.is_some() {
            p { class: "settings-status", role: "status", "data-testid": "cleanup-review-empty",
                "No suggestions pending."
            }
        }
    }
}

/// One suggestion rendered as a card: the action sentence, the names it acts
/// on, how many books it touches, and (for authors) the cached photo.
#[component]
fn SuggestionCardView(card: SuggestionCard) -> Element {
    let books = if card.book_count == 1 {
        "1 book".to_string()
    } else {
        format!("{} books", card.book_count)
    };
    rsx! {
        article { class: "cleanup-card", "data-testid": "cleanup-card",
            if let Some(photo) = card.photo_url.clone() {
                img {
                    class: "cleanup-card-photo",
                    src: "{photo}",
                    alt: "",
                    "data-testid": "cleanup-card-photo",
                }
            }
            p { class: "cleanup-card-action", "data-testid": "cleanup-card-action",
                "{action_sentence(card.kind, card.action)}"
            }
            p { class: "cleanup-card-primary", "data-testid": "cleanup-card-primary",
                "{card.primary_name}"
            }
            if let Some(secondary) = card.secondary_name.clone() {
                p { class: "cleanup-card-secondary", "data-testid": "cleanup-card-secondary",
                    "{secondary_label(card.action)}: {secondary}"
                }
            }
            p { class: "cleanup-card-books", "data-testid": "cleanup-card-books", "{books} affected" }
        }
    }
}

/// The one-line description of what accepting this card does.
fn action_sentence(kind: CleanupKind, action: CleanupAction) -> &'static str {
    match (kind, action) {
        (CleanupKind::Author, CleanupAction::Merge) => "Merge these authors into one.",
        (CleanupKind::Author, CleanupAction::Delete) => "Delete this author record.",
        (CleanupKind::Series, CleanupAction::Merge) => "Merge these series into one.",
        (CleanupKind::Tag, CleanupAction::Merge) => "Merge these tags into one.",
        (CleanupKind::Tag, CleanupAction::Split) => "Split this tag into its parts.",
        (CleanupKind::BookTitle, CleanupAction::Rename) => "Adopt the normalized title.",
        _ => "Apply this cleanup.",
    }
}

/// What the secondary name means for this action.
fn secondary_label(action: CleanupAction) -> &'static str {
    match action {
        CleanupAction::Rename => "Proposed",
        _ => "Merging away",
    }
}

#[cfg(all(test, feature = "server"))]
mod tests;
