//! Tests for the mobile audiobook player's pure helpers: time formatting,
//! locating the chapter at an elapsed position, remaining-time math at a
//! playback rate, and prev/next chapter and part seek targets.

use super::*;

fn ch(ordinal: i64, title: &str, start: f64, dur: f64) -> ChapterInfo {
    ChapterInfo {
        ordinal,
        title: title.into(),
        start_seconds: start,
        duration_seconds: dur,
    }
}

#[test]
fn format_hms_under_one_hour_renders_mm_ss() {
    assert_eq!(format_hms(0.0), "0:00");
    assert_eq!(format_hms(5.0), "0:05");
    assert_eq!(format_hms(65.0), "1:05");
    assert_eq!(format_hms(599.9), "9:59");
}

#[test]
fn format_hms_past_one_hour_renders_h_mm_ss() {
    assert_eq!(format_hms(3600.0), "1:00:00");
    assert_eq!(format_hms(3661.0), "1:01:01");
    assert_eq!(format_hms(13_596.0), "3:46:36");
}

#[test]
fn format_hms_handles_negative_and_non_finite_as_zero() {
    assert_eq!(format_hms(-12.0), "0:00");
    assert_eq!(format_hms(f64::NAN), "0:00");
    assert_eq!(format_hms(f64::INFINITY), "0:00");
}

#[test]
fn format_ms_stays_in_minutes_past_an_hour() {
    assert_eq!(format_ms(0.0), "0:00");
    assert_eq!(format_ms(90.0), "1:30");
    assert_eq!(format_ms(3661.0), "61:01");
    assert_eq!(format_ms(-5.0), "0:00");
}

#[test]
fn format_hm_renders_hours_and_minutes() {
    assert_eq!(format_hm(47.0 * 60.0), "47m");
    assert_eq!(format_hm(13.0 * 3600.0 + 52.0 * 60.0), "13h 52m");
}

#[test]
fn chapter_index_for_elapsed_returns_zero_for_empty_list() {
    assert_eq!(chapter_index_for_elapsed(&[], 60.0), 0);
}

#[test]
fn chapter_index_for_elapsed_tracks_boundaries() {
    let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
    assert_eq!(chapter_index_for_elapsed(&chs, 0.0), 0);
    assert_eq!(chapter_index_for_elapsed(&chs, 150.0), 0);
    assert_eq!(chapter_index_for_elapsed(&chs, 300.0), 1);
    assert_eq!(chapter_index_for_elapsed(&chs, 900.0), 1);
}

#[test]
fn remaining_in_chapter_counts_down_to_zero() {
    let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
    assert!((remaining_in_chapter(&chs, 0, 100.0) - 200.0).abs() < f64::EPSILON);
    assert!((remaining_in_chapter(&chs, 1, 300.0) - 600.0).abs() < f64::EPSILON);
    // Past the end clamps to zero, and OOB is zero.
    assert_eq!(remaining_in_chapter(&chs, 0, 400.0), 0.0);
    assert_eq!(remaining_in_chapter(&chs, 9, 0.0), 0.0);
}

#[test]
fn chapter_prev_seek_none_when_empty_or_oob() {
    assert_eq!(chapter_prev_seek(&[], 10.0, 0), None);
    let chs = vec![ch(1, "Intro", 0.0, 300.0)];
    assert_eq!(chapter_prev_seek(&chs, 1.0, 5), None);
}

#[test]
fn chapter_prev_seek_restarts_current_when_well_into_it() {
    let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
    assert_eq!(chapter_prev_seek(&chs, 350.0, 1), Some(300.0));
}

#[test]
fn chapter_prev_seek_goes_back_when_near_start() {
    let chs = vec![ch(1, "Intro", 0.0, 300.0), ch(2, "Part 1", 300.0, 600.0)];
    assert_eq!(chapter_prev_seek(&chs, 301.0, 1), Some(0.0));
    assert_eq!(chapter_prev_seek(&chs, 1.0, 0), Some(0.0));
}

#[test]
fn next_part_index_advances_then_stops_at_end() {
    assert_eq!(next_part_index(3, 0), Some(1));
    assert_eq!(next_part_index(3, 1), Some(2));
    assert_eq!(next_part_index(3, 2), None);
    assert_eq!(next_part_index(1, 0), None);
}

#[test]
fn part_token_url_appends_token_with_correct_separator() {
    assert_eq!(
        part_token_url("http://h:3000", "/api/audiobooks/x/parts/0", Some("tok")),
        "http://h:3000/api/audiobooks/x/parts/0?token=tok"
    );
    // Existing query → `&token=`.
    assert_eq!(
        part_token_url(
            "http://h:3000",
            "/api/audiobooks/x/parts/0?file_id=5",
            Some("t")
        ),
        "http://h:3000/api/audiobooks/x/parts/0?file_id=5&token=t"
    );
    // No token → bare origin-prefixed URL.
    assert_eq!(
        part_token_url("http://h:3000", "/api/audiobooks/x/parts/0", None),
        "http://h:3000/api/audiobooks/x/parts/0"
    );
}

#[test]
fn player_view_from_direct_derives_display_fields() {
    let mut book = EbookMetadata {
        title: Some("A Sea of Glass and Fire".into()),
        filename: "book.m4b".into(),
        accent: Some("#c74".into()),
        ..Default::default()
    };
    book.creators.push(omnibus_shared::Contributor {
        name: "Jane Doe".into(),
        ..Default::default()
    });
    let chs = vec![ch(1, "One", 0.0, 60.0)];
    let v = PlayerView::from_direct(&book, chs, 13.0 * 3600.0 + 52.0 * 60.0, Vec::new());
    assert_eq!(v.title, "A Sea of Glass and Fire");
    assert_eq!(v.author, "Jane Doe");
    assert_eq!(v.accent.as_deref(), Some("#c74"));
    assert_eq!(v.total_label, "13h 52m");
    assert_eq!(v.chapters.len(), 1);
}

#[test]
fn player_view_falls_back_to_filename_and_unknown_author() {
    let book = EbookMetadata {
        title: Some("   ".into()),
        filename: "raw.m4b".into(),
        ..Default::default()
    };
    let v = PlayerView::from_hls(&book);
    assert_eq!(v.title, "raw.m4b");
    assert_eq!(v.author, "Unknown Author");
    assert!(v.chapters.is_empty());
    assert!(v.parts.is_empty());
}
