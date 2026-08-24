//! SSR-rendered coverage of the sync readout line: the unlinked nudge, the
//! stale warning, and the linked "one spot, both formats" line with its
//! audio-timeline position (saved, or mapped from the reading percent).

use dioxus::prelude::*;
use omnibus_shared::{
    AlignmentAudioFile, AlignmentAudioPosition, AlignmentLink, AlignmentPosition, AlignmentView,
    CrossFormatLinkMode,
};

use super::sync_line;

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

/// Signal-hosting harness: `sync_line` takes a live `Signal`, so it renders
/// inside a real `VirtualDom` via a one-prop host component.
#[component]
fn SyncLineHost(view: AlignmentView) -> Element {
    let open = use_signal(|| false);
    sync_line(&view, open)
}

fn render(view: &AlignmentView) -> String {
    crate::test_support::render(rsx! {
        SyncLineHost { view: view.clone() }
    })
}

#[test]
fn sync_line_offers_link_formats_when_unlinked() {
    let html = render(&base_view());
    // SSR escapes the apostrophe.
    assert!(html.contains("positions aren&#39;t synced"), "{html}");
    assert!(html.contains("data-testid=\"sync-link-open\""), "{html}");
}

#[test]
fn sync_line_warns_and_offers_review_when_stale() {
    let mut view = base_view();
    view.link = Some(AlignmentLink {
        stale: true,
        ..fresh_link()
    });
    let html = render(&view);
    assert!(html.contains("sync is paused"), "{html}");
    assert!(html.contains("data-testid=\"sync-link-review\""), "{html}");
}

#[test]
fn sync_line_names_the_saved_audio_position_when_linked() {
    let mut view = base_view();
    view.link = Some(fresh_link());
    view.listening = Some(AlignmentAudioPosition {
        book_file_id: Some(1),
        seconds: 1800.0,
        client_updated_at: 200,
    });
    let html = render(&view);
    assert!(html.contains("one spot, both formats"), "{html}");
    assert!(html.contains("audio at 30m"), "{html}");
    assert!(html.contains("data-testid=\"sync-link-manage\""), "{html}");
}

#[test]
fn sync_line_maps_the_reading_percent_onto_the_timeline_without_a_listening_position() {
    let mut view = base_view();
    view.link = Some(fresh_link());
    view.reading = Some(AlignmentPosition {
        percent: Some(40),
        client_updated_at: 100,
    });
    let html = render(&view);
    // Linear default mapping: text 40% of a 1h timeline → 24m.
    assert!(html.contains("audio at 24m"), "{html}");
}
