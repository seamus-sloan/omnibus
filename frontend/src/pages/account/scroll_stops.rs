//! "Omnibus User Settings" account card (Settings → Account): the table of
//! per-user feature switches, the book detail's scroll stops being the first.
//! A table because the list is expected to grow — a new switch is a row, not
//! a section — and each row saves on change, since independent switches have
//! nothing to batch a Save across.

use dioxus::prelude::*;

use crate::data;

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
    mut viewer_slot: Signal<Option<Option<omnibus_shared::UserSummary>>>,
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
                    // Patch the app-wide viewer so a book detail page opened
                    // next reads the new value — without this the context
                    // keeps the stale one and the page renders the layout the
                    // reader just turned off, until a reload.
                    //
                    // Written from the value the server just accepted rather
                    // than re-fetched: a failed refetch would leave the
                    // context stale with nothing to report, and this is the
                    // one field whose new value is already known here.
                    viewer_slot.with_mut(|slot| {
                        if let Some(Some(user)) = slot.as_mut() {
                            user.book_detail_scroll_stops = next;
                        }
                    });
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

    // Captured before the async save: `use_current_user` is a hook and can
    // only run during render.
    let viewer_slot = crate::use_current_user().0;
    let toggle = scroll_stops_toggle_handler(confirmed, shown, error, saving, viewer_slot);
    let is_on = shown() == Some(true);

    rsx! {
        section { class: "card", "data-testid": "account-scroll-stops-card",
            h2 { "Omnibus User Settings" }
            p { class: "subtitle", "Features you can turn on for your account alone." }

            table { class: "users-table settings-table", "data-testid": "user-settings-table",
                thead {
                    tr {
                        th { "Setting" }
                        th { class: "settings-col-switch", "Enabled" }
                    }
                }
                tbody {
                    tr { "data-testid": "user-setting-scroll-stops",
                        td {
                            div { class: "settings-row-name", "Book details scroll stops" }
                            div { class: "settings-row-note",
                                "{scroll_stops_status_line(confirmed())}"
                            }
                        }
                        td { class: "settings-col-switch",
                            label { class: "settings-switch",
                                input {
                                    r#type: "checkbox",
                                    role: "switch",
                                    "aria-label": "Use book details scroll stops",
                                    "data-testid": "scroll-stops-toggle",
                                    checked: is_on,
                                    disabled: confirmed().is_none() || saving(),
                                    onchange: toggle,
                                }
                            }
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
