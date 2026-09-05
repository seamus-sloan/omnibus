//! SSR-rendered coverage of the sync readout line: the unlinked nudge, the
//! stale warning, the linked "one spot, both formats" line with its
//! audio-timeline position (saved, or mapped from the reading percent), and
//! the follow switch that line carries.

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
    sync_line("book-uuid", &view, open, EventHandler::new(move |_| {}))
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
    // Named like a control, not a trailing "link formats \u{2192}" run of prose.
    assert!(html.contains("Link Formats"), "{html}");
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
    // The affordance names what it manages rather than reading "manage →".
    assert!(html.contains("Manage Ebook &#38; Audiobook Sync"), "{html}");
}

#[test]
fn sync_line_renders_the_follow_switch_unchecked_when_follow_is_off() {
    let mut view = base_view();
    view.link = Some(fresh_link());
    let html = render(&view);
    assert!(
        html.contains("data-testid=\"sync-follow-toggle\""),
        "{html}"
    );
    assert!(html.contains("role=\"switch\""), "{html}");
    assert!(html.contains("aria-checked=\"false\""), "{html}");
    assert!(html.contains("not following"), "{html}");
}

#[test]
fn sync_line_renders_the_follow_switch_checked_when_follow_is_on() {
    let mut view = base_view();
    view.link = Some(AlignmentLink {
        follow: true,
        ..fresh_link()
    });
    let html = render(&view);
    assert!(html.contains("aria-checked=\"true\""), "{html}");
    // The off label is a superstring of the on label — assert the switch
    // isn't merely rendering "not following" and matching on the tail.
    assert!(!html.contains("not following"), "{html}");
}

#[test]
fn sync_line_has_no_follow_switch_without_a_link() {
    // Nothing to follow until the alignment is confirmed, and the server
    // would 409 the flip; the unlinked and stale lines offer the modal.
    let unlinked = render(&base_view());
    assert!(
        !unlinked.contains("data-testid=\"sync-follow-toggle\""),
        "{unlinked}"
    );

    let mut view = base_view();
    view.link = Some(AlignmentLink {
        stale: true,
        ..fresh_link()
    });
    let stale = render(&view);
    assert!(
        !stale.contains("data-testid=\"sync-follow-toggle\""),
        "{stale}"
    );
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
