//! The self-registration switch rendered above the Users table, plus its
//! pure load/toggle logic split out so a test can exercise the guards and
//! the failure-revert without a browser.

use dioxus::prelude::*;

use crate::data;

/// Subtitle describing what the current self-registration setting means, in
/// terms of who can create an account rather than restating the switch.
fn registration_status_line(enabled: Option<bool>) -> &'static str {
    match enabled {
        None => "Checking…",
        Some(true) => "Anyone who can reach this server can create an account.",
        Some(false) => "Only an administrator can create accounts.",
    }
}

/// `onchange` for the self-registration switch, extracted from
/// [`RegistrationToggle`] so its guards and its failure revert are reachable
/// from a test without a browser.
///
/// Ignores the event: the click already flipped the DOM checkbox, and the
/// value this writes comes from `confirmed`, not from the input.
fn registration_toggle_handler(
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
            match data::set_registration_enabled(next).await {
                Ok(()) => {
                    confirmed.set(Some(next));
                    error.set(None);
                }
                Err(e) => {
                    // Push the checkbox back to what the server still holds.
                    shown.set(Some(current));
                    error.set(Some(e));
                }
            }
            saving.set(false);
        });
    }
}

/// Applies the outcome of a self-registration status fetch to the toggle's
/// signals. Free function (rather than inlined in the effect) so both the
/// initial load and a retry drive the exact same match, and so a test can
/// exercise the `Err` branch without spawning an async task — mirrors
/// [`registration_toggle_handler`].
fn apply_registration_load(
    result: Result<bool, String>,
    mut confirmed: Signal<Option<bool>>,
    mut shown: Signal<Option<bool>>,
    mut error: Signal<Option<String>>,
) {
    match result {
        Ok(open) => {
            confirmed.set(Some(open));
            shown.set(Some(open));
            error.set(None);
        }
        Err(e) => error.set(Some(e)),
    }
}

/// Self-registration switch. Renders above the users table because it governs
/// who may join without an admin creating the account first.
///
/// Both signals start `None` so SSR and the first WASM paint emit the same
/// markup (rule 07); the checkbox is disabled until the load settles, so the
/// control can never be flipped against a value that hasn't arrived.
///
/// `confirmed` is the value the server has acknowledged and drives the
/// subtitle, which therefore never describes a policy that isn't in force.
/// `shown` is what the checkbox renders. They are separate because a click
/// flips the DOM checkbox *natively*: if the rendered value never moved,
/// Dioxus would diff it as unchanged, emit no patch, and a rejected save would
/// leave the box sitting in a state the server refused.
///
/// A failed initial load must not brick the control forever: `reload` is
/// read by the effect (so it fires once on mount) and bumped by the "Retry"
/// affordance shown while `error` is set and `confirmed` never arrived —
/// clicking it re-runs the same fetch rather than leaving the checkbox
/// `disabled` until a full page reload.
///
/// `reload` doubles as a generation counter: each spawned fetch snapshots
/// the value it was started with and, on completion, only applies its
/// result if `reload` still reads the same value. Without that guard a
/// slow first load and a fast retry can land out of order — the retry's
/// success would be immediately clobbered by the original request's later
/// (stale) failure.
#[component]
pub(super) fn RegistrationToggle() -> Element {
    let confirmed = use_signal(|| Option::<bool>::None);
    let shown = use_signal(|| Option::<bool>::None);
    let error = use_signal(|| None::<String>);
    let saving = use_signal(|| false);
    let mut reload = use_signal(|| 0u32);

    use_effect(move || {
        let generation = reload();
        spawn(async move {
            let result = data::registration_status().await;
            // A newer load (another mount-effect run, or a Retry click) has
            // since started — this one is superseded, so its result (which
            // may be a stale failure) must not overwrite what that newer
            // load already applied or is about to apply.
            if reload() == generation {
                apply_registration_load(result, confirmed, shown, error);
            }
        });
    });

    let toggle = registration_toggle_handler(confirmed, shown, error, saving);

    let is_on = shown() == Some(true);
    let status = registration_status_line(confirmed());
    let can_retry = error().is_some() && confirmed().is_none();

    rsx! {
        section { class: "card", "data-testid": "registration-card",
            div { class: "users-head",
                div {
                    h2 { "Self-registration" }
                    p { class: "subtitle", "{status}" }
                }
                label { class: "auth-checkbox",
                    input {
                        r#type: "checkbox",
                        "data-testid": "registration-toggle",
                        checked: is_on,
                        disabled: confirmed().is_none() || saving(),
                        onchange: toggle,
                    }
                    span { "Allow new users to register" }
                }
            }
            if let Some(err) = error() {
                p {
                    role: "alert",
                    class: "settings-status error",
                    "data-testid": "registration-error",
                    "{err}"
                }
            }
            if can_retry {
                button {
                    r#type: "button",
                    class: "btn ghost sm",
                    "data-testid": "registration-retry",
                    onclick: move |_| reload.with_mut(|n| *n += 1),
                    "Retry"
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
