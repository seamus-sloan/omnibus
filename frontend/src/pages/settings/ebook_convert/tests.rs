//! Tests for the format-conversion settings card: the pure display helpers,
//! and a render assertion on the unresolved first paint (rendered output
//! contains the expected content, per `.claude/rules/03-unit-testing.md`'s
//! frontend guidance).

use super::*;

fn status(source: &str, path: &str, available: bool) -> EbookConvertStatus {
    EbookConvertStatus {
        available,
        path: path.to_string(),
        source: source.to_string(),
    }
}

// ---------- pure display helpers ----------

#[test]
fn placeholder_for_falls_back_to_the_bare_path_name_before_the_status_lands() {
    // Not a distro-specific absolute path: the backend resolves the bare name
    // on `$PATH`, and a macOS Calibre lives nowhere near /usr/bin.
    assert_eq!(placeholder_for(None), DEFAULT_EBOOK_CONVERT_BIN);
    assert_eq!(placeholder_for(None), "ebook-convert");
}

#[test]
fn placeholder_for_shows_the_resolved_command_once_the_status_lands() {
    let s = status("settings", "/opt/calibre/ebook-convert", true);
    assert_eq!(placeholder_for(Some(&s)), "/opt/calibre/ebook-convert");
}

#[test]
fn is_overridden_is_true_only_for_a_saved_settings_value() {
    assert!(is_overridden(Some(&status("settings", "/x", true))));
    assert!(!is_overridden(Some(&status("env", "/x", true))));
    assert!(!is_overridden(Some(&status("path", "ebook-convert", true))));
    assert!(!is_overridden(None));
}

#[test]
fn status_detail_is_empty_until_the_status_lands() {
    assert_eq!(status_detail(None), "");
}

#[test]
fn status_detail_names_the_source_and_the_resolved_path() {
    let s = status("env", "/usr/local/bin/ebook-convert", true);
    assert_eq!(
        status_detail(Some(&s)),
        "Detected \u{00b7} env \u{00b7} /usr/local/bin/ebook-convert"
    );
}

#[test]
fn save_outcome_message_reports_a_runnable_path_as_success() {
    let (text, is_error) = save_outcome_message(true);
    assert_eq!(text, "ebook-convert path saved.");
    assert!(!is_error);
}

#[test]
fn save_outcome_message_flags_a_saved_but_unrunnable_path_as_an_error() {
    let (text, is_error) = save_outcome_message(false);
    assert!(
        text.contains("not runnable"),
        "expected a not-runnable warning, got: {text}"
    );
    assert!(is_error, "a path the server can't run is not a success");
}

// ---------- render (SSR / first paint) ----------

#[test]
fn card_renders_the_unresolved_state_before_the_status_loads() {
    // SSR and the first WASM paint must emit the same unresolved card or
    // hydration mis-adopts it (rule 07). `rebuild_in_place` never polls the
    // load effect's future, so this is exactly that first paint.
    let html = crate::test_support::render_in_vdom(EbookConvertField);
    assert!(
        html.contains("data-testid=\"ebook-convert-card\""),
        "card must render, got: {html}"
    );
    assert!(html.contains("Format conversion"), "got: {html}");
    assert!(
        html.contains("placeholder=\"ebook-convert\""),
        "unresolved placeholder must be the bare $PATH name, got: {html}"
    );
    assert!(
        html.contains("Not detected"),
        "an unresolved/unavailable binary reads as Not detected, not an error, got: {html}"
    );
}

#[test]
fn card_hides_use_auto_detection_until_an_override_is_saved() {
    // The button only has something to clear when `source == "settings"`, and
    // the unresolved first paint has no status at all.
    let html = crate::test_support::render_in_vdom(EbookConvertField);
    assert!(
        !html.contains("ebook-convert-clear"),
        "no override is saved on first paint, got: {html}"
    );
    assert!(!html.contains("Use auto-detection"), "got: {html}");
}
