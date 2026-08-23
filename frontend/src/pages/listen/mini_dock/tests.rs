//! Unit tests for the mini-dock's pure display and popover-state helpers.

use omnibus_shared::EbookMetadata;

use super::meta::{dock_active_state, dock_is_active, dock_sub_text, progress_pct, time_left_text};
use super::popovers::{chip_class, toggle_panel, DockPanel};

#[test]
fn progress_pct_is_zero_when_duration_unknown() {
    assert_eq!(progress_pct(10.0, 0.0), 0.0);
    assert_eq!(progress_pct(10.0, -5.0), 0.0);
}

#[test]
fn progress_pct_computes_midpoint() {
    assert!((progress_pct(50.0, 200.0) - 25.0).abs() < f64::EPSILON);
}

#[test]
fn progress_pct_clamps_above_full() {
    assert_eq!(progress_pct(300.0, 200.0), 100.0);
}

#[test]
fn progress_pct_treats_non_finite_elapsed_as_zero() {
    assert_eq!(progress_pct(f64::NAN, 100.0), 0.0);
    assert_eq!(progress_pct(f64::INFINITY, 100.0), 0.0);
}

#[test]
fn time_left_text_is_none_when_duration_unknown() {
    assert_eq!(time_left_text(10.0, 0.0, 1.0), None);
    assert_eq!(time_left_text(f64::NAN, 100.0, 1.0), None);
}

#[test]
fn time_left_text_formats_hours_and_minutes() {
    // 6600 s left at 1.0× → 110 min → 1h 50m.
    assert_eq!(
        time_left_text(0.0, 6600.0, 1.0),
        Some("about 1h 50m left".to_string())
    );
}

#[test]
fn time_left_text_formats_minutes_only_under_an_hour() {
    assert_eq!(
        time_left_text(0.0, 1800.0, 1.0),
        Some("about 30m left".to_string())
    );
}

#[test]
fn time_left_text_divides_by_playback_rate() {
    // 3600 s left at 2.0× → 30 wall-clock minutes.
    assert_eq!(
        time_left_text(0.0, 3600.0, 2.0),
        Some("about 30m left".to_string())
    );
}

#[test]
fn time_left_text_is_none_at_or_past_the_end() {
    // At the finish there is no time left — don't claim a phantom "1m".
    assert_eq!(time_left_text(3600.0, 3600.0, 1.0), None);
    assert_eq!(time_left_text(4000.0, 3600.0, 1.0), None);
}

#[test]
fn time_left_text_floors_at_one_minute_and_ignores_bad_rate() {
    assert_eq!(
        time_left_text(3599.0, 3600.0, 1.0),
        Some("about 1m left".to_string())
    );
    // Non-finite / non-positive rates fall back to 1.0×.
    assert_eq!(
        time_left_text(0.0, 1800.0, f64::NAN),
        Some("about 30m left".to_string())
    );
    assert_eq!(
        time_left_text(0.0, 1800.0, 0.0),
        Some("about 30m left".to_string())
    );
}

#[test]
fn dock_sub_text_includes_chapter_clock_and_time_left() {
    let sub = dock_sub_text(Some("Ch. 2 \u{00b7} The Journey".into()), 65.0, 3665.0, 1.0);
    assert_eq!(
        sub,
        "Ch. 2 \u{00b7} The Journey \u{00b7} 1:05 / 1:01:05 \u{00b7} about 1h 0m left"
    );
}

#[test]
fn dock_sub_text_is_clock_and_time_left_without_chapters() {
    assert_eq!(
        dock_sub_text(None, 65.0, 185.0, 1.0),
        "1:05 / 3:05 \u{00b7} about 2m left"
    );
}

#[test]
fn dock_sub_text_scales_the_clock_to_the_playback_rate() {
    // 10 book-minutes into an hour at 2x: the clock and the time-left share
    // one wall-clock basis (5:00 elapsed + 25m left = the 30:00 total),
    // rather than a 1x clock beside a rate-adjusted "left" (#2108).
    assert_eq!(
        dock_sub_text(None, 600.0, 3600.0, 2.0),
        "5:00 / 30:00 \u{00b7} about 25m left"
    );
}

// Mirrors MiniDock's "renders empty host vs. active bar" branch without needing a full component render.
#[test]
fn dock_active_state_is_none_when_nothing_is_playing() {
    assert_eq!(dock_active_state(None, None), None);
}

#[test]
fn dock_active_state_is_none_when_uuid_has_not_resolved_yet() {
    let book = EbookMetadata {
        title: Some("Test Audiobook".into()),
        ..Default::default()
    };
    assert_eq!(dock_active_state(Some(book), None), None);
}

#[test]
fn dock_active_state_is_none_when_book_has_not_loaded_yet() {
    assert_eq!(dock_active_state(None, Some("uuid-1".to_string())), None);
}

// The `/read` route reserves reflow space off `dock_is_active`; it must
// agree with `dock_active_state` (the dock's own render gate) on every
// combination or the reader reserves space for a bar that isn't there.
#[test]
fn dock_is_active_agrees_with_dock_active_state_for_every_combination() {
    let book = EbookMetadata {
        title: Some("Test Audiobook".into()),
        ..Default::default()
    };
    for (b, u) in [
        (None, None),
        (Some(book.clone()), None),
        (None, Some("uuid-1".to_string())),
        (Some(book.clone()), Some("uuid-1".to_string())),
    ] {
        assert_eq!(
            dock_is_active(&b, &u),
            dock_active_state(b.clone(), u.clone()).is_some()
        );
    }
}

#[test]
fn dock_active_state_returns_both_when_book_and_uuid_are_present() {
    let book = EbookMetadata {
        title: Some("Test Audiobook".into()),
        ..Default::default()
    };
    let (got_book, got_uuid) =
        dock_active_state(Some(book.clone()), Some("uuid-1".to_string())).unwrap();
    assert_eq!(got_book, book);
    assert_eq!(got_uuid, "uuid-1");
}

#[test]
fn toggle_panel_opens_a_closed_panel() {
    assert_eq!(toggle_panel(None, DockPanel::Speed), Some(DockPanel::Speed));
}

#[test]
fn toggle_panel_closes_the_open_panel_on_repeat_click() {
    assert_eq!(toggle_panel(Some(DockPanel::Speed), DockPanel::Speed), None);
}

#[test]
fn toggle_panel_switches_between_panels() {
    assert_eq!(
        toggle_panel(Some(DockPanel::Speed), DockPanel::Sleep),
        Some(DockPanel::Sleep)
    );
    assert_eq!(
        toggle_panel(Some(DockPanel::Sleep), DockPanel::Volume),
        Some(DockPanel::Volume)
    );
}

#[test]
fn chip_class_appends_on_only_while_open() {
    assert_eq!(chip_class("mini-dock-chip", false), "mini-dock-chip");
    assert_eq!(chip_class("mini-dock-chip", true), "mini-dock-chip on");
}
