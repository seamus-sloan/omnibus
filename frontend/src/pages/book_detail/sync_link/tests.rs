//! SSR-rendered coverage of the "Your progress" rows: per-format bars
//! while unlinked or stale, the single newest-format row + mapped
//! footnote once linked and fresh.

use omnibus_shared::{
    AlignmentAudioFile, AlignmentAudioPosition, AlignmentLink, AlignmentPosition, AlignmentView,
    CrossFormatLinkMode,
};

use super::progress_rows;

fn base_view() -> AlignmentView {
    AlignmentView {
        link: None,
        anchor_match: None,
        ebook: None,
        audio_files: vec![AlignmentAudioFile {
            book_file_id: 1,
            label: "part1.m4b".into(),
            duration_seconds: 3600.0,
            chapter_starts: vec![],
        }],
        reading: None,
        listening: None,
        anchor_pairs: vec![],
        audio_chapter_marks: 0,
    }
}

fn fresh_link() -> AlignmentLink {
    AlignmentLink {
        mode: CrossFormatLinkMode::Sequence,
        primary_book_file_id: None,
        stale: false,
        confirmed_at: 0,
        follow: false,
        user_anchors: 0,
    }
}

fn reading(percent: i64, clock: i64) -> AlignmentPosition {
    AlignmentPosition {
        percent: Some(percent),
        client_updated_at: clock,
    }
}

fn listening(seconds: f64, clock: i64) -> AlignmentAudioPosition {
    AlignmentAudioPosition {
        book_file_id: Some(1),
        seconds,
        client_updated_at: clock,
    }
}

fn render(view: &AlignmentView) -> String {
    dioxus::ssr::render_element(progress_rows(view))
}

#[test]
fn progress_rows_renders_both_rows_when_unlinked_with_both_positions() {
    let mut view = base_view();
    view.reading = Some(reading(55, 100));
    view.listening = Some(listening(1800.0, 200));
    let html = render(&view);
    assert!(html.contains("data-testid=\"bd-progress-ebook\""), "{html}");
    assert!(html.contains("data-testid=\"bd-progress-audio\""), "{html}");
    assert!(html.contains("55%"), "{html}");
    assert!(html.contains("30m of 1h 00m"), "{html}");
}

#[test]
fn progress_rows_skips_the_ebook_row_without_a_reading_position() {
    let mut view = base_view();
    view.listening = Some(listening(1800.0, 200));
    let html = render(&view);
    assert!(
        !html.contains("data-testid=\"bd-progress-ebook\""),
        "{html}"
    );
    assert!(html.contains("data-testid=\"bd-progress-audio\""), "{html}");
}

#[test]
fn progress_rows_renders_the_newest_audio_row_with_mapped_page_footnote_when_linked() {
    let mut view = base_view();
    view.link = Some(fresh_link());
    view.reading = Some(reading(40, 100));
    view.listening = Some(listening(1800.0, 200));
    let html = render(&view);
    assert!(
        html.contains("data-testid=\"bd-progress-newest\""),
        "{html}"
    );
    assert!(html.contains("newest: audiobook"), "{html}");
    // Linear default mapping: audio 50% → text 50%.
    assert!(
        html.contains("the reader opens at the matching page (\u{2248} 50%)"),
        "{html}"
    );
    assert!(
        !html.contains("data-testid=\"bd-progress-ebook\""),
        "{html}"
    );
}

#[test]
fn progress_rows_renders_the_newest_ebook_row_with_mapped_time_footnote_when_linked() {
    let mut view = base_view();
    view.link = Some(fresh_link());
    view.reading = Some(reading(40, 300));
    view.listening = Some(listening(1800.0, 200));
    let html = render(&view);
    assert!(
        html.contains("data-testid=\"bd-progress-newest\""),
        "{html}"
    );
    assert!(html.contains("newest: ebook"), "{html}");
    // Linear default mapping: text 40% of a 1h timeline → 24m.
    assert!(
        html.contains("the player opens at the matching time (\u{2248} 24m)"),
        "{html}"
    );
}

#[test]
fn progress_rows_falls_back_to_per_format_rows_when_the_link_is_stale() {
    let mut view = base_view();
    view.link = Some(AlignmentLink {
        stale: true,
        ..fresh_link()
    });
    view.reading = Some(reading(55, 100));
    view.listening = Some(listening(1800.0, 200));
    let html = render(&view);
    assert!(html.contains("data-testid=\"bd-progress-ebook\""), "{html}");
    assert!(html.contains("data-testid=\"bd-progress-audio\""), "{html}");
    assert!(
        !html.contains("data-testid=\"bd-progress-newest\""),
        "{html}"
    );
}

#[test]
fn progress_rows_renders_nothing_without_positions() {
    let html = render(&base_view());
    assert!(!html.contains("bd-prog-row"), "{html}");
}
