//! Tests for the Users settings section's date helpers: `fmt_date`
//! formatting known epochs in UTC and staying stable within a civil day,
//! and `civil_from_days` round-tripping the epoch.

use super::*;

#[test]
fn fmt_date_formats_known_epochs_utc() {
    assert_eq!(fmt_date(0), "Jan 1, 1970");
    // 2024-01-01T00:00:00Z
    assert_eq!(fmt_date(1_704_067_200), "Jan 1, 2024");
    // 2026-07-25T00:00:00Z
    assert_eq!(fmt_date(1_784_937_600), "Jul 25, 2026");
}

#[test]
fn fmt_date_is_stable_within_a_day() {
    // Any second within the civil day maps to the same date (SSR/hydration
    // must not disagree because of sub-day drift).
    assert_eq!(fmt_date(1_704_067_200), fmt_date(1_704_067_200 + 86_399));
}

#[test]
fn civil_from_days_round_trips_epoch() {
    assert_eq!(civil_from_days(0), (1970, 1, 1));
}

// ── Self-registration switch ──────────────────────────────────────────

#[test]
fn registration_status_line_describes_who_can_create_an_account() {
    assert_eq!(
        registration_status_line(Some(true)),
        "Anyone who can reach this server can create an account."
    );
    assert_eq!(
        registration_status_line(Some(false)),
        "Only an administrator can create accounts."
    );
}

#[test]
fn registration_status_line_is_noncommittal_before_the_load_settles() {
    // The unresolved state must not claim either policy — the switch is
    // disabled until it settles, and asserting the wrong one would be worse
    // than saying nothing.
    let pending = registration_status_line(None);
    assert_eq!(pending, "Checking…");
    assert_ne!(pending, registration_status_line(Some(true)));
    assert_ne!(pending, registration_status_line(Some(false)));
}

// `test_support`'s SSR renderer only exists on the `server` feature.
#[cfg(feature = "server")]
#[test]
fn registration_toggle_renders_unresolved_and_disabled_before_load() {
    // SSR (and the first WASM paint) must both emit the unresolved state, or
    // hydration mis-adopts the checkbox — rule 07. `rebuild_in_place` never
    // polls the load effect's future, so this is exactly that first paint.
    let html = crate::test_support::render_in_vdom(RegistrationToggle);
    assert!(html.contains("registration-toggle"), "toggle must render");
    assert!(
        html.contains("disabled"),
        "the switch must be inert until the current value arrives, got: {html}"
    );
    assert!(
        html.contains("Checking"),
        "expected the unresolved subtitle, got: {html}"
    );
}

// `registration_toggle_handler` runs inside a `VirtualDom::new(...)
// .rebuild_in_place()` so `Signal::new` has a runtime — the pattern in
// `frontend/src/pages/settings/library/tests.rs`. The futures it spawns are
// never polled, so no request actually fires; these assert the synchronous
// guards and the pre-spawn state the handler commits to.

#[cfg(feature = "server")]
fn blank_form_event() -> Event<FormData> {
    use std::rc::Rc;

    use dioxus::prelude::SerializedFormData;

    Event::new(
        Rc::new(FormData::new(SerializedFormData::new(
            String::new(),
            vec![],
        ))),
        false,
    )
}

#[cfg(feature = "server")]
#[test]
fn registration_toggle_handler_ignores_a_click_before_the_value_loads() {
    #[component]
    fn AssertIgnores() -> Element {
        let confirmed = Signal::new(None::<bool>);
        let shown = Signal::new(None::<bool>);
        let error = Signal::new(None::<String>);
        let saving = Signal::new(false);

        let mut handler = registration_toggle_handler(confirmed, shown, error, saving);
        handler(blank_form_event());

        // Nothing may be sent before we know what we'd be changing *from* —
        // the switch is also `disabled` in this state, so this is the
        // belt-and-braces half of the same guard.
        assert!(!saving(), "must not start a save with no loaded value");
        assert_eq!(shown(), None, "the checkbox must stay unresolved");

        rsx! {}
    }
    VirtualDom::new(AssertIgnores).rebuild_in_place();
}

#[cfg(feature = "server")]
#[test]
fn registration_toggle_handler_ignores_a_click_while_a_save_is_in_flight() {
    #[component]
    fn AssertIgnores() -> Element {
        let confirmed = Signal::new(Some(true));
        let shown = Signal::new(Some(true));
        let error = Signal::new(None::<String>);
        let saving = Signal::new(true);

        let mut handler = registration_toggle_handler(confirmed, shown, error, saving);
        handler(blank_form_event());

        // A second click must not queue a second write against a value the
        // first one may be about to change.
        assert_eq!(shown(), Some(true), "in-flight save must swallow the click");

        rsx! {}
    }
    VirtualDom::new(AssertIgnores).rebuild_in_place();
}

#[cfg(feature = "server")]
#[test]
fn registration_toggle_handler_moves_the_checkbox_but_not_the_subtitle() {
    #[component]
    fn AssertSplit() -> Element {
        let confirmed = Signal::new(Some(false));
        let shown = Signal::new(Some(false));
        let error = Signal::new(None::<String>);
        let saving = Signal::new(false);

        let mut handler = registration_toggle_handler(confirmed, shown, error, saving);
        handler(blank_form_event());

        // `shown` tracks the native DOM flip so a later revert is a real vdom
        // change; `confirmed` must not move until the server answers, so the
        // subtitle never describes a policy that isn't in force yet.
        assert_eq!(shown(), Some(true), "checkbox must track the native flip");
        assert_eq!(
            confirmed(),
            Some(false),
            "subtitle must still describe the server's value"
        );
        assert_eq!(
            registration_status_line(confirmed()),
            "Only an administrator can create accounts."
        );
        assert!(saving(), "the switch must be inert while the write is out");

        rsx! {}
    }
    VirtualDom::new(AssertSplit).rebuild_in_place();
}
