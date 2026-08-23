//! Tests for the mobile player's per-render display derivation — pins the
//! scrubber row's one-basis contract at the `derive_player_state` boundary.

use omnibus_shared::{ChapterInfo, EbookMetadata};

use super::derive_player_state;
use super::state::SleepState;
use super::view::PlayerView;

/// Two 30-minute chapters; the values every test below derives from.
fn two_chapter_view() -> PlayerView {
    let chapters = vec![
        ChapterInfo {
            ordinal: 1,
            title: "One".into(),
            start_seconds: 0.0,
            duration_seconds: 1800.0,
        },
        ChapterInfo {
            ordinal: 2,
            title: "Two".into(),
            start_seconds: 1800.0,
            duration_seconds: 1800.0,
        },
    ];
    PlayerView::from_direct(&EbookMetadata::default(), chapters, 3600.0, vec![])
}

#[test]
fn derive_player_state_scales_every_readout_to_the_playback_rate() {
    // 20 book-minutes into the first chapter at 2x: elapsed 10:00, chapter
    // 10:00-in / 15:00-total / 5:00-left, book 10:00-left — the row sums on
    // one wall-clock basis (#2108).
    let d = derive_player_state(
        &two_chapter_view(),
        1200.0,
        3600.0,
        0,
        2.0,
        SleepState::Off,
        None,
    );
    assert!((d.elapsed_book - 600.0).abs() < f64::EPSILON);
    assert!((d.within - 600.0).abs() < f64::EPSILON);
    assert!((d.chapter_dur - 900.0).abs() < f64::EPSILON);
    assert!((d.chapter_left - 300.0).abs() < f64::EPSILON);
    assert!((d.remaining_book - 1200.0).abs() < f64::EPSILON);
    assert!((d.within + d.chapter_left - d.chapter_dur).abs() < f64::EPSILON);
    assert!((d.elapsed_book + d.remaining_book - 1800.0).abs() < f64::EPSILON);
    // The seek coordinates stay 1x book-time — the range input's value/max,
    // not readouts.
    assert!((d.effective - 1200.0).abs() < f64::EPSILON);
    assert!((d.scrub_max - 3600.0).abs() < f64::EPSILON);
}

#[test]
fn derive_player_state_keeps_book_time_labels_at_1x() {
    let d = derive_player_state(
        &two_chapter_view(),
        1200.0,
        3600.0,
        0,
        1.0,
        SleepState::Off,
        None,
    );
    assert!((d.elapsed_book - 1200.0).abs() < f64::EPSILON);
    assert!((d.within - 1200.0).abs() < f64::EPSILON);
    assert!((d.chapter_dur - 1800.0).abs() < f64::EPSILON);
    assert!((d.chapter_left - 600.0).abs() < f64::EPSILON);
    assert!((d.remaining_book - 2400.0).abs() < f64::EPSILON);
}
