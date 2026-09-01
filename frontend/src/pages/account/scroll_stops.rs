//! "Book details" account card: whether the caller's book detail page uses
//! the snap-stop marquee or reads as one continuous scroll. Off by default.
//! A single switch, saved on change rather than behind a Save button — with
//! nothing else in the card to batch with, a button would only add a step
//! between the click and the setting taking effect.
//!
//! The switch is parked: nothing reads the preference yet, so it renders
//! disabled behind a "Coming soon" marker rather than letting a reader set a
//! value that changes nothing. The save path below is wired and tested, so
//! lighting it up is dropping [`PARKED`] and the marker beside it.

use dioxus::prelude::*;

use crate::data;

/// Whether the switch is inert. `true` until the book detail pages actually
/// branch on the stored preference — see the module docs.
const PARKED: bool = true;

/// `onchange` for the scroll-stops switch, extracted so its guards and its
/// failure revert are reachable from a test without a browser. Mirrors the
/// self-registration toggle in `pages/settings/users/registration.rs`.
///
/// Ignores the event: the click already flipped the DOM checkbox, and the
/// value this writes comes from `confirmed`, not from the input.
fn scroll_stops_toggle_handler(
    confirmed: Signal<Option<bool>>,
    mut shown: Signal<Option<bool>>,
    mut error: Signal<Option<String>>,
    mut saving: Signal<bool>,
) -> impl FnMut(Event<FormData>) {
    move |_| {
        let Some(current) = confirmed() else {
            return;
        };
        if saving() {
            return;
        }
        saving.set(true);
        // Track the native flip so a later revert is a real vdom change.
        let next = !current;
        shown.set(Some(next));
        let mut confirmed = confirmed;
        spawn(async move {
            match data::set_book_detail_scroll_stops("", next).await {
                Ok(()) => {
                    confirmed.set(Some(next));
                    error.set(None);
                }
                Err(e) => {
                    // Push the checkbox back to what the server still holds.
                    shown.set(Some(current));
                    error.set(Some(e.to_string()));
                }
            }
            saving.set(false);
        });
    }
}

/// Subtitle describing what the setting currently does, in terms of how the
/// page reads rather than restating the switch.
fn scroll_stops_status_line(enabled: Option<bool>) -> &'static str {
    if PARKED {
        return "Book details scroll continuously, top to bottom.";
    }
    match enabled {
        None => "Checking…",
        Some(true) => "Book details snap through one panel at a time.",
        Some(false) => "Book details scroll continuously, top to bottom.",
    }
}

/// The book-detail scroll-stops card (Settings → Account).
///
/// Both signals start `None` so SSR and the first WASM paint emit the same
/// markup (rule 07), and the switch stays disabled until the viewer resolves
/// so it can never be flipped against a value that hasn't arrived.
/// `confirmed` is what the server has acknowledged and drives the subtitle;
/// `shown` is what the checkbox renders. They are separate because a click
/// flips the DOM checkbox natively — if the rendered value never moved,
/// Dioxus would diff it as unchanged and a rejected save would leave the box
/// sitting in a state the server refused.
#[cfg(not(feature = "mobile"))]
#[component]
pub(crate) fn ScrollStopsCard() -> Element {
    let confirmed = use_signal(|| None::<bool>);
    let shown = use_signal(|| None::<bool>);
    let error = use_signal(|| None::<String>);
    let saving = use_signal(|| false);

    // Seed once from the resolved viewer. A later context refresh (another
    // card's save) must not clobber a value this card has since changed, so
    // the seed only ever fires while `confirmed` is still unresolved.
    let viewer = crate::use_current_user_summary();
    use_effect(move || {
        if confirmed.peek().is_some() {
            return;
        }
        if let Some(user) = viewer() {
            let mut confirmed = confirmed;
            let mut shown = shown;
            confirmed.set(Some(user.book_detail_scroll_stops));
            shown.set(Some(user.book_detail_scroll_stops));
        }
    });

    let toggle = scroll_stops_toggle_handler(confirmed, shown, error, saving);
    let is_on = shown() == Some(true);

    rsx! {
        section { class: "card", "data-testid": "account-scroll-stops-card",
            div { class: "users-head",
                div {
                    h2 { "Book details" }
                    p { class: "subtitle", "{scroll_stops_status_line(confirmed())}" }
                }
                label { class: "auth-checkbox",
                    input {
                        r#type: "checkbox",
                        "data-testid": "scroll-stops-toggle",
                        checked: is_on,
                        disabled: PARKED || confirmed().is_none() || saving(),
                        onchange: toggle,
                    }
                    span { "Use book details scroll stops" }
                    if PARKED {
                        span { class: "settings-soon", "data-testid": "scroll-stops-soon",
                            "Coming soon"
                        }
                    }
                }
            }
            if let Some(err) = error() {
                p {
                    role: "alert",
                    class: "settings-status error",
                    "data-testid": "scroll-stops-error",
                    "{err}"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
