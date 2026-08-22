use super::*;

#[test]
fn snap_rate_clamps_and_snaps_to_five_hundredths() {
    assert!((snap_rate(0.1) - 0.5).abs() < 1e-9);
    assert!((snap_rate(9.0) - 3.0).abs() < 1e-9);
    assert!((snap_rate(1.23) - 1.25).abs() < 1e-9);
    assert!((snap_rate(1.2) - 1.2).abs() < 1e-9);
}

#[test]
fn sleep_preset_on_matches_off_and_armed_presets() {
    assert!(sleep_preset_on(SleepState::Off, 0));
    assert!(!sleep_preset_on(SleepState::Off, 900));
    let armed = SleepState::Countdown {
        remaining: 100,
        preset: 900,
    };
    assert!(sleep_preset_on(armed, 900));
    assert!(!sleep_preset_on(armed, 1800));
    assert!(!sleep_preset_on(
        SleepState::EndOfChapter { at_seconds: 1.0 },
        0
    ));
}

// `VirtualDom`-render tests for the sheet primitives: build a zero-prop
// harness component (mirrors `contexts::tests::AssertBustCounter`), drive
// it through a real `VirtualDom`, and assert on the SSR'd HTML. This file
// only compiles under `mobile` (via the parent `mobile` module's
// `#![cfg(feature = "mobile")]`), so these already exercise mobile-only
// markup; the `server` gate below is what additionally brings in
// `dioxus::ssr` to serialize it — run with
// `cargo test -p omnibus-frontend --features mobile,server` (see
// `crate::test_support` for the pattern writeup).
#[cfg(feature = "server")]
mod render {
    use super::*;
    use crate::test_support::render_in_vdom;

    fn chapter(ordinal: i64, title: &str, start: f64, dur: f64) -> ChapterInfo {
        ChapterInfo {
            ordinal,
            title: title.into(),
            start_seconds: start,
            duration_seconds: dur,
        }
    }

    #[test]
    fn m_sheet_renders_title_testid_and_children_with_no_meta() {
        fn harness() -> Element {
            rsx! {
                MSheet {
                    title: "Chapters".to_string(),
                    testid: "mobile-test-sheet".to_string(),
                    meta: None,
                    on_close: move |_| {},
                    div { "data-testid": "sheet-body", "Body content" }
                }
            }
        }
        let html = render_in_vdom(harness);
        assert!(html.contains("data-testid=\"mobile-test-sheet\""));
        assert!(html.contains("Chapters"));
        assert!(html.contains("data-testid=\"sheet-body\""));
        assert!(html.contains("Body content"));
    }

    #[test]
    fn m_sheet_renders_the_meta_slot_when_provided() {
        fn harness() -> Element {
            rsx! {
                MSheet {
                    title: "Playback speed".to_string(),
                    testid: "mobile-speed-sheet".to_string(),
                    meta: rsx! { span { "1.00\u{00d7}" } },
                    on_close: move |_| {},
                    div {}
                }
            }
        }
        let html = render_in_vdom(harness);
        assert!(html.contains("1.00\u{00d7}"));
    }

    #[test]
    fn chapters_sheet_renders_every_chapter_and_the_count_meta() {
        fn harness() -> Element {
            let list = ChaptersListView {
                chapters: vec![
                    chapter(1, "Intro", 0.0, 300.0),
                    chapter(2, "Part One", 300.0, 600.0),
                ],
                current_index: 0,
                elapsed: 100.0,
                total_label: "15m".to_string(),
            };
            rsx! {
                ChaptersSheet { list, on_seek: move |_| {}, on_close: move |_| {} }
            }
        }
        let html = render_in_vdom(harness);
        assert!(html.contains("Intro"));
        assert!(html.contains("Part One"));
        assert!(html.contains("2 \u{00b7} 15m"));
    }

    // Simulates the "chapter advanced" event: `super::view::chapter_index_for_elapsed`
    // is what a real playback tick would feed into `current_index`, so
    // re-rendering with it bumped is the VirtualDom-level stand-in for
    // that state change. Asserts the highlighted row and the completed
    // marker actually move rather than just checking each render in
    // isolation.
    #[test]
    fn chapters_sheet_moves_the_highlight_when_the_current_chapter_advances() {
        fn list_at(current_index: usize) -> ChaptersListView {
            ChaptersListView {
                chapters: vec![
                    chapter(1, "Intro", 0.0, 300.0),
                    chapter(2, "Part One", 300.0, 600.0),
                ],
                current_index,
                elapsed: 0.0,
                total_label: "15m".to_string(),
            }
        }
        fn harness_first() -> Element {
            rsx! {
                ChaptersSheet { list: list_at(0), on_seek: move |_| {}, on_close: move |_| {} }
            }
        }
        fn harness_second() -> Element {
            rsx! {
                ChaptersSheet { list: list_at(1), on_seek: move |_| {}, on_close: move |_| {} }
            }
        }
        let first = render_in_vdom(harness_first);
        let second = render_in_vdom(harness_second);
        assert_ne!(first, second);
        // At index 0 nothing is "done" yet; at index 1 the first chapter
        // has been passed and picks up the check mark.
        assert!(!first.contains("m-ch-trail m-ch-done"));
        assert!(second.contains("m-ch-trail m-ch-done"));

        // The `current` row class and the playing marker sit on whichever
        // row is `current_index`. HTML preserves chapter order (Intro,
        // then Part One), so "current appears before Part One" /
        // "current appears after Intro" pins it to the right row without
        // needing a DOM query.
        assert!(first.contains("m-ch-trail m-ch-playing"));
        let first_current = first
            .find("m-ch-row current")
            .expect("current_index 0 should mark a row current");
        let first_part_one = first.find("Part One").expect("Part One should render");
        assert!(
            first_current < first_part_one,
            "at current_index 0 the current row should be Intro, before Part One"
        );

        assert!(second.contains("m-ch-trail m-ch-playing"));
        let second_current = second
            .find("m-ch-row current")
            .expect("current_index 1 should mark a row current");
        let second_intro = second.find("Intro").expect("Intro should render");
        assert!(
            second_current > second_intro,
            "at current_index 1 the current row should be Part One, after Intro"
        );
    }
}
