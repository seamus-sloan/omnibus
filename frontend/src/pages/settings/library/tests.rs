//! Tests for the pure library-summary formatter, the settings-save
//! rescan-interval guard, and the "Scan Library" maintenance handler.
//!
//! `Signal::new`/`spawn` need a live Dioxus runtime, and `save_settings_handler`
//! takes a real `FormEvent` (`scan_library_handler` a real `MouseEvent`), so
//! those two run inside a throwaway zero-prop component driven by
//! `VirtualDom::new(...).rebuild_in_place()` — the pattern established in
//! `frontend/src/pages/reader/prefs/tests.rs` and `frontend/src/contexts.rs`.
//! The handlers only ever call `prevent_default()`/ignore the event, so a
//! bare `SerializedFormData`/`SerializedMouseData` is enough — no real
//! browser event is needed. Neither test lets the spawned save/scan future
//! run (it's queued but never polled), so no network call actually fires;
//! each test only observes the synchronous guard/in-flight behavior before
//! the request would be sent.

use std::rc::Rc;

use dioxus::prelude::*;

use super::*;

fn section(total: usize, counts: &[(&str, usize)]) -> LibrarySection {
    LibrarySection {
        path: Some("/lib".into()),
        total_files: total,
        counts_by_ext: counts.iter().map(|(e, c)| (e.to_string(), *c)).collect(),
        error: None,
    }
}

#[test]
fn summary_line_reports_total_only_when_no_ext_breakdown() {
    assert_eq!(library_summary_line(&section(3, &[])), "3 file(s) found.");
}

#[test]
fn summary_line_appends_a_clause_per_extension() {
    let line = library_summary_line(&section(5, &[("epub", 4), ("pdf", 1)]));
    assert!(line.starts_with("5 file(s) found."));
    assert!(line.contains("4 .epub found."));
    assert!(line.contains("1 .pdf found."));
}

/// A no-op `FormEvent` — `save_settings_handler` never reads the form's own
/// values, so a blank `SerializedFormData` is enough to call it.
fn blank_form_event() -> FormEvent {
    Event::new(
        Rc::new(FormData::new(SerializedFormData::new(
            String::new(),
            vec![],
        ))),
        false,
    )
}

/// A no-op `MouseEvent` — `scan_library_handler` ignores its argument.
fn blank_mouse_event() -> MouseEvent {
    Event::new(
        Rc::new(MouseData::new(SerializedMouseData::default())),
        false,
    )
}

#[test]
fn save_settings_handler_rejects_non_numeric_interval_without_saving() {
    #[component]
    fn AssertRejects() -> Element {
        let ebook_path = Signal::new(String::new());
        let audiobook_path = Signal::new(String::new());
        let scan_interval_hours = Signal::new("soon".to_string());
        let status = Signal::new(None::<String>);
        let status_is_error = Signal::new(false);
        let library_refresh = Signal::new(0u32);

        let mut handler = save_settings_handler(
            "http://127.0.0.1:0".to_string(),
            ebook_path,
            audiobook_path,
            scan_interval_hours,
            status,
            status_is_error,
            library_refresh,
        );
        handler(blank_form_event());

        assert_eq!(
            status(),
            Some("Automatic rescan interval must be a whole number of hours.".to_string())
        );
        assert!(status_is_error());
        // The guard returned before spawning the save, so the
        // only-bumped-on-success refresh counter is untouched.
        assert_eq!(library_refresh(), 0);

        rsx! {}
    }
    VirtualDom::new(AssertRejects).rebuild_in_place();
}

#[test]
fn save_settings_handler_accepts_a_blank_interval_and_proceeds_to_save() {
    #[component]
    fn AssertAccepts() -> Element {
        let ebook_path = Signal::new(String::new());
        let audiobook_path = Signal::new(String::new());
        let scan_interval_hours = Signal::new(String::new());
        let status = Signal::new(None::<String>);
        let status_is_error = Signal::new(false);
        let library_refresh = Signal::new(0u32);

        let mut handler = save_settings_handler(
            "http://127.0.0.1:0".to_string(),
            ebook_path,
            audiobook_path,
            scan_interval_hours,
            status,
            status_is_error,
            library_refresh,
        );
        handler(blank_form_event());

        // The guard didn't reject a blank interval, so no rejection message
        // was set synchronously — it went on to spawn the actual save,
        // whose result lands asynchronously (covered end-to-end by
        // Playwright's settings flow).
        assert_eq!(status(), None);
        assert!(!status_is_error());

        rsx! {}
    }
    VirtualDom::new(AssertAccepts).rebuild_in_place();
}

#[test]
fn scan_library_handler_flips_in_flight_immediately_on_click() {
    #[component]
    fn AssertScanInFlight() -> Element {
        let status = Signal::new(None::<String>);
        let status_is_error = Signal::new(false);
        let scan_in_flight = Signal::new(false);
        let library_refresh = Signal::new(0u32);

        let mut handler = scan_library_handler(
            "http://127.0.0.1:0".to_string(),
            status,
            status_is_error,
            scan_in_flight,
            library_refresh,
        );
        assert!(!scan_in_flight());
        handler(blank_mouse_event());

        // The in-flight flag flips synchronously, before the scan request
        // itself ever resolves — that's what disables the button on click.
        assert!(scan_in_flight());

        rsx! {}
    }
    VirtualDom::new(AssertScanInFlight).rebuild_in_place();
}
