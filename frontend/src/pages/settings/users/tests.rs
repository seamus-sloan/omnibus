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
