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
mod tests;
