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

// ── Retry after a failed initial load ───────────────────────────────────
//
// `data::registration_status()` is stubbed to always return `Ok(true)` under
// `--features server` (the SSR/native test target never actually runs the
// web fetch — see the stub's doc comment in `frontend/src/data/auth.rs`), so
// these drive `apply_registration_status` and `registration_retry_handler`
// directly with a synthetic `Result` rather than a real network failure.

#[test]
fn needs_registration_retry_is_true_only_when_load_failed_and_never_resolved() {
    assert!(!needs_registration_retry(None, None));
    assert!(needs_registration_retry(Some(&"boom".to_string()), None));
    // A stale error alongside an already-confirmed value shouldn't occur in
    // practice (`apply_registration_status` clears `error` on success), but
    // the predicate must still favor "resolved" if it ever does.
    assert!(!needs_registration_retry(
        Some(&"boom".to_string()),
        Some(true)
    ));
    assert!(!needs_registration_retry(None, Some(false)));
}

#[cfg(feature = "server")]
#[test]
fn apply_registration_status_sets_error_and_leaves_the_checkbox_unresolved_on_err() {
    #[component]
    fn AssertErr() -> Element {
        let confirmed = Signal::new(None::<bool>);
        let shown = Signal::new(None::<bool>);
        let error = Signal::new(None::<String>);

        apply_registration_status(confirmed, shown, error, Err("network error".to_string()));

        assert_eq!(
            confirmed(),
            None,
            "an errored load must not resolve a value"
        );
        assert_eq!(shown(), None);
        assert_eq!(error(), Some("network error".to_string()));
        assert!(
            needs_registration_retry(error().as_ref(), confirmed()),
            "the failure must leave the retry affordance visible"
        );

        rsx! {}
    }
    VirtualDom::new(AssertErr).rebuild_in_place();
}

#[cfg(feature = "server")]
#[test]
fn apply_registration_status_clears_a_prior_error_and_enables_the_checkbox_on_retry_success() {
    #[component]
    fn AssertRetrySucceeds() -> Element {
        // Start where a failed initial load leaves things: unresolved and
        // errored — this is the state the new retry button appears in.
        let confirmed = Signal::new(None::<bool>);
        let shown = Signal::new(None::<bool>);
        let error = Signal::new(Some("network error".to_string()));

        apply_registration_status(confirmed, shown, error, Ok(true));

        assert_eq!(
            confirmed(),
            Some(true),
            "retry must resolve the confirmed value"
        );
        assert_eq!(shown(), Some(true), "retry must re-enable the checkbox");
        assert_eq!(
            error(),
            None,
            "a successful retry must clear the prior error"
        );
        assert!(!needs_registration_retry(error().as_ref(), confirmed()));

        rsx! {}
    }
    VirtualDom::new(AssertRetrySucceeds).rebuild_in_place();
}

/// A no-op `MouseEvent` — `registration_retry_handler` ignores its argument.
#[cfg(feature = "server")]
fn blank_mouse_event() -> MouseEvent {
    use dioxus::prelude::SerializedMouseData;

    Event::new(
        std::rc::Rc::new(MouseData::new(SerializedMouseData::default())),
        false,
    )
}

#[cfg(feature = "server")]
#[test]
fn registration_retry_handler_flips_retrying_immediately_on_click() {
    #[component]
    fn AssertRetryInFlight() -> Element {
        let confirmed = Signal::new(None::<bool>);
        let shown = Signal::new(None::<bool>);
        let error = Signal::new(Some("network error".to_string()));
        let retrying = Signal::new(false);

        let mut handler = registration_retry_handler(confirmed, shown, error, retrying);
        handler(blank_mouse_event());

        // Disables synchronously, before the retried fetch resolves — same
        // shape as `scan_library_handler_flips_in_flight_immediately_on_click`.
        assert!(retrying());

        rsx! {}
    }
    VirtualDom::new(AssertRetryInFlight).rebuild_in_place();
}

#[cfg(feature = "server")]
#[test]
fn registration_retry_handler_ignores_a_click_while_a_retry_is_in_flight() {
    #[component]
    fn AssertIgnores() -> Element {
        let confirmed = Signal::new(None::<bool>);
        let shown = Signal::new(None::<bool>);
        let error = Signal::new(Some("network error".to_string()));
        let retrying = Signal::new(true);

        let mut handler = registration_retry_handler(confirmed, shown, error, retrying);
        handler(blank_mouse_event());

        // A second click must not queue a second fetch on top of the one
        // already in flight.
        assert!(retrying());

        rsx! {}
    }
    VirtualDom::new(AssertIgnores).rebuild_in_place();
}
