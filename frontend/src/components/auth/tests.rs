//! SSR render-smoke tests for the auth-page primitives. Each renders one
//! component with representative props through the shared `test_support::render`
//! harness and asserts the visible content / ARIA shape reaches the markup —
//! catching render panics and broken conditionals the pure-helper unit tests
//! in the sibling files can't reach.

use dioxus::prelude::*;

use super::{
    AuthShell, Banner, BannerKind, Field, PasswordRequirements, StrengthMeter, StrengthScore,
};
use crate::test_support::render;

#[test]
fn field_renders_label_bound_to_input_and_shows_its_error_alert() {
    let html = render(rsx! {
        Field {
            label: "Password",
            input_id: "password",
            error: Some("Too short".to_string()),
            input { id: "password", r#type: "password" }
        }
    });

    assert!(html.contains("data-testid=\"password-field\""));
    assert!(html.contains("for=\"password\""));
    assert!(html.contains("Password"));
    // The error copy lands in a sibling alert, and the wrapper takes the
    // error visual — not the success one.
    assert!(html.contains("role=\"alert\""));
    assert!(html.contains("Too short"));
    assert!(html.contains("auth-field-err"));
}

#[test]
fn field_hint_shows_when_there_is_no_error() {
    let html = render(rsx! {
        Field {
            label: "Username",
            input_id: "username",
            hint: Some("Letters and numbers".to_string()),
            input { id: "username" }
        }
    });

    assert!(html.contains("auth-field-msg-hint"));
    assert!(html.contains("Letters and numbers"));
    assert!(!html.contains("role=\"alert\""));
}

#[test]
fn banner_error_uses_alert_role_and_renders_title_and_message() {
    let html = render(rsx! {
        Banner {
            kind: BannerKind::Err,
            title: "Sign-in failed",
            message: "Check your password.".to_string(),
        }
    });

    assert!(html.contains("auth-banner-err"));
    assert!(html.contains("role=\"alert\""));
    assert!(html.contains("Sign-in failed"));
    assert!(html.contains("Check your password."));
}

#[test]
fn banner_info_uses_status_role_and_a_dismiss_button_when_dismissible() {
    let html = render(rsx! {
        Banner {
            kind: BannerKind::Info,
            title: "Heads up",
            dismissible: true,
        }
    });

    assert!(html.contains("auth-banner-info"));
    assert!(html.contains("role=\"status\""));
    assert!(html.contains("aria-label=\"Dismiss\""));
    assert!(html.contains("Heads up"));
}

#[test]
fn strength_meter_fills_segments_up_to_the_score_and_renders_its_label() {
    let html = render(rsx! {
        StrengthMeter { score: StrengthScore::new(4), label: Some("strong".to_string()) }
    });

    assert!(html.contains("role=\"meter\""));
    assert!(html.contains("aria-valuenow=\"4\""));
    assert!(html.contains("auth-strength-tier-ok"));
    assert!(html.contains("auth-strength-segment-on"));
    assert!(html.contains("strong"));
}

#[test]
fn password_requirements_marks_each_rule_met_or_unmet() {
    let html = render(rsx! {
        PasswordRequirements { rules: [true, false, true] }
    });

    assert!(html.contains("At least 10 characters"));
    assert!(html.contains("Mixed case"));
    assert!(html.contains("One number or symbol"));
    // Screen-reader status text distinguishes met from unmet.
    assert!(html.contains(": met"));
    assert!(html.contains(": not met"));
}

#[test]
fn auth_shell_wraps_form_children_with_the_branded_kicker_and_title() {
    let html = render(rsx! {
        AuthShell {
            kicker: "Sign in",
            title: rsx! { "Welcome back" },
            lede: "Pick up where you left off.".to_string(),
            button { r#type: "submit", "Sign in" }
        }
    });

    assert!(html.contains("auth-shell-kicker"));
    assert!(html.contains("Sign in"));
    assert!(html.contains("Welcome back"));
    assert!(html.contains("Pick up where you left off."));
    // The form children are slotted into the body column.
    assert!(html.contains("auth-shell-body"));
    assert!(html.contains("Omnibus"));
}
