//! Bottom sheets for the mobile player: the shared sheet frame plus the
//! chapters, playback-speed, and sleep-timer sheets. The bookmarks sheet
//! (which owns transport state) lives in [`super::bookmarks_sheet`].

use dioxus::prelude::*;
use omnibus_shared::ChapterInfo;

use super::state::{format_countdown, sleep_remaining, SleepState, SLEEP_PRESETS};
use super::view::format_ms;

/// Speed presets shown in the sheet grid, mirroring the web `SpeedPanel`.
use omnibus_shared::{
    MAX_AUDIOBOOK_PLAYBACK_RATE as SPEED_MAX, MIN_AUDIOBOOK_PLAYBACK_RATE as SPEED_MIN,
};

const SPEED_PRESETS: &[f64] = &[0.5, 0.8, 1.0, 1.1, 1.2, 1.5, 1.8, 2.0];
/// Fine-tune slider bounds + step (also the ± stepper increment).
const SPEED_STEP: f64 = 0.05;

/// Clamp + snap a requested rate to the fine-tune grid.
pub(super) fn snap_rate(rate: f64) -> f64 {
    let clamped = rate.clamp(SPEED_MIN, SPEED_MAX);
    (clamped / SPEED_STEP).round() * SPEED_STEP
}

/// Shared bottom-sheet frame: scrim, sheet body, grabber, and a title row
/// with an optional right-side meta element. Children render below the head.
#[component]
pub(super) fn MSheet(
    title: String,
    testid: String,
    meta: Option<Element>,
    on_close: EventHandler<MouseEvent>,
    children: Element,
) -> Element {
    rsx! {
        div { class: "m-sheet-scrim", "data-testid": testid, onclick: move |e| on_close.call(e),
            div { class: "m-sheet", onclick: move |e| e.stop_propagation(),
                div { class: "m-sheet-grabber" }
                div { class: "m-sheet-head",
                    h4 { "{title}" }
                    if let Some(meta) = meta {
                        {meta}
                    }
                }
                {children}
            }
        }
    }
}

/// Render-ready view data for the chapters sheet: the chapter map plus the
/// playback position that highlights the current row.
#[derive(Clone, PartialEq)]
pub(super) struct ChaptersListView {
    /// Ordered chapter markers rendered as sheet rows.
    pub chapters: Vec<ChapterInfo>,
    /// Index of the currently-playing chapter.
    pub current_index: usize,
    /// Seconds elapsed in the whole book (drives the current row's bar).
    pub elapsed: f64,
    /// Formatted total-duration label shown in the sheet header.
    pub total_label: String,
}

/// Props for the [`ChaptersSheet`] component: the view data plus handlers.
#[derive(Props, Clone, PartialEq)]
pub(super) struct ChaptersSheetProps {
    pub list: ChaptersListView,
    /// Fired with the target time in seconds when a row is tapped.
    pub on_seek: EventHandler<f64>,
    /// Fired when the scrim dismisses the sheet.
    pub on_close: EventHandler<MouseEvent>,
}

/// Bottom-sheet chapter list. Current row is accent-highlighted with a mini
/// progress bar; done rows show a check, upcoming rows show their duration.
#[component]
pub(super) fn ChaptersSheet(props: ChaptersSheetProps) -> Element {
    let ChaptersSheetProps {
        list:
            ChaptersListView {
                chapters,
                current_index,
                elapsed,
                total_label,
            },
        on_seek,
        on_close,
    } = props;
    let count = chapters.len();
    rsx! {
        MSheet {
            title: "Chapters",
            testid: "mobile-chapters-sheet",
            meta: rsx! {
                span { class: "mono m-sheet-count", "{count} \u{00b7} {total_label}" }
            },
            on_close,
            div { class: "m-sheet-list",
                for (i, ch) in chapters.iter().enumerate() {
                    {chapter_row(i, ch, current_index, elapsed, &on_seek)}
                }
            }
        }
    }
}

/// One chapter row in the sheet.
fn chapter_row(
    i: usize,
    ch: &ChapterInfo,
    current_index: usize,
    elapsed: f64,
    on_seek: &EventHandler<f64>,
) -> Element {
    let is_current = i == current_index;
    let is_done = i < current_index;
    let start = ch.start_seconds;
    let handler = *on_seek;
    let progress = if is_current && ch.duration_seconds > 0.0 {
        (((elapsed - start) / ch.duration_seconds).clamp(0.0, 1.0) * 100.0) as i64
    } else {
        0
    };
    rsx! {
        button {
            key: "{ch.ordinal}",
            class: if is_current { "m-ch-row current" } else { "m-ch-row" },
            r#type: "button",
            onclick: move |_| handler.call(start),
            span { class: "mono m-ch-num", "{ch.ordinal}" }
            div { class: "m-ch-body",
                div { class: "m-ch-title", span { class: "m-em", "{ch.title}" } }
                if is_current {
                    div { class: "pbar m-ch-pbar", i { style: "width:{progress}%" } }
                }
            }
            if is_current {
                span { class: "m-ch-trail m-ch-playing", "\u{25B6}" }
            } else if is_done {
                span { class: "m-ch-trail m-ch-done", "\u{2713}" }
            } else {
                span { class: "mono m-ch-trail m-ch-dur", "{format_ms(ch.duration_seconds)}" }
            }
        }
    }
}

/// Playback-speed sheet: preset grid, fine-tune slider, and a ±0.05 stepper.
#[component]
pub(super) fn SpeedSheet(
    rate: f64,
    on_set: EventHandler<f64>,
    on_close: EventHandler<MouseEvent>,
) -> Element {
    let rate_big = format!("{rate:.2}\u{00d7}");
    let stepper_label = format!("{rate:.2}\u{00d7}");
    let on_input = move |evt: Event<FormData>| {
        if let Ok(v) = evt.value().parse::<f64>() {
            on_set.call(snap_rate(v));
        }
    };
    rsx! {
        MSheet {
            title: "Playback speed",
            testid: "mobile-speed-sheet",
            meta: rsx! {
                span { class: "mono m-sheet-meta-num", "{rate_big}" }
            },
            on_close,
            div { class: "m-sheet-body",
                div { class: "m-sheet-grid cols-4",
                    for &preset in SPEED_PRESETS {
                        button {
                            key: "{preset}",
                            r#type: "button",
                            class: if (preset - rate).abs() < 0.001 { "m-sheet-opt mono on" } else { "m-sheet-opt mono" },
                            onclick: move |_| on_set.call(preset),
                            "{preset:.1}\u{00d7}"
                        }
                    }
                }
                div { class: "m-speed-fine",
                    div { class: "m-speed-fine-labels",
                        span { class: "label", "Fine-tune" }
                        span { class: "label", "0.5\u{00d7} \u{2014} 3.0\u{00d7}" }
                    }
                    input {
                        class: "m-player-range m-speed-range",
                        r#type: "range",
                        "aria-label": "Playback speed",
                        min: "{SPEED_MIN}",
                        max: "{SPEED_MAX}",
                        step: "{SPEED_STEP}",
                        value: "{rate}",
                        oninput: on_input,
                    }
                }
                div { class: "m-speed-stepper",
                    button {
                        r#type: "button", class: "m-sheet-opt m-speed-step",
                        "aria-label": "Slower",
                        onclick: move |_| on_set.call(snap_rate(rate - SPEED_STEP)),
                        "\u{2212}"
                    }
                    div { class: "m-speed-step-readout",
                        div { class: "mono", "{stepper_label}" }
                        div { class: "label", "0.05 steps" }
                    }
                    button {
                        r#type: "button", class: "m-sheet-opt m-speed-step",
                        "aria-label": "Faster",
                        onclick: move |_| on_set.call(snap_rate(rate + SPEED_STEP)),
                        "+"
                    }
                }
            }
        }
    }
}

/// Timer state the sleep sheet renders from: the armed state, the playback
/// position (for the live countdown), and the current chapter's boundary.
#[derive(Clone, Copy, PartialEq)]
pub(super) struct SleepSheetView {
    pub sleep: SleepState,
    pub elapsed: f64,
    /// End of the currently-playing chapter (absolute seconds), when known.
    pub chapter_end: Option<f64>,
    /// 1-based chapter number for the "End of chapter N" label.
    pub chapter_no: usize,
}

/// Sleep-timer sheet: preset grid plus an "End of chapter" option, with the
/// live remaining countdown in the header while armed.
#[component]
pub(super) fn SleepSheet(
    view: SleepSheetView,
    on_set: EventHandler<SleepState>,
    on_close: EventHandler<MouseEvent>,
) -> Element {
    let SleepSheetView {
        sleep,
        elapsed,
        chapter_end,
        chapter_no,
    } = view;
    let remaining = sleep_remaining(sleep, elapsed);
    let meta = remaining.filter(|r| *r > 0).map(|r| {
        rsx! {
            span { class: "m-sheet-meta-stack",
                span { class: "mono m-sheet-meta-num", "{format_countdown(r)}" }
                span { class: "label", "remaining" }
            }
        }
    });
    let eoc_on = matches!(sleep, SleepState::EndOfChapter { .. });
    rsx! {
        MSheet {
            title: "Sleep timer",
            testid: "mobile-sleep-sheet",
            meta,
            on_close,
            div { class: "m-sheet-body",
                div { class: "m-sheet-grid cols-2",
                    for &(label, secs) in SLEEP_PRESETS {
                        button {
                            key: "{label}",
                            r#type: "button",
                            class: if sleep_preset_on(sleep, secs) { "m-sheet-opt on" } else { "m-sheet-opt" },
                            onclick: move |_| {
                                on_set.call(if secs <= 0 {
                                    SleepState::Off
                                } else {
                                    SleepState::Countdown { remaining: secs, preset: secs }
                                });
                            },
                            "{label}"
                        }
                    }
                }
                if let Some(end) = chapter_end {
                    button {
                        r#type: "button",
                        class: if eoc_on { "m-sheet-opt m-sleep-eoc on" } else { "m-sheet-opt m-sleep-eoc" },
                        onclick: move |_| on_set.call(SleepState::EndOfChapter { at_seconds: end }),
                        span { class: "m-sleep-eoc-dot" }
                        "End of chapter {chapter_no}"
                    }
                }
            }
        }
    }
}

/// Whether a preset button should render highlighted for the current state.
fn sleep_preset_on(sleep: SleepState, secs: i32) -> bool {
    match sleep {
        SleepState::Off => secs == 0,
        SleepState::Countdown { preset, .. } => preset == secs,
        SleepState::EndOfChapter { .. } => false,
    }
}

#[cfg(test)]
mod tests {
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
        }
    }
}
