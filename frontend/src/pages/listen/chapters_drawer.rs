//! Chapters drawer for the listen page.
//!
//! Right-side full-height panel showing the table of contents with
//! played / current / upcoming states. Clicking a row seeks to that
//! chapter's start position.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::ChapterInfo;

use super::helpers::{format_hms, remaining_at_rate};
use super::panel_shell::ListenDrawerShell;

/// One chapter row: ordinal/checkmark/play glyph, title, and
/// duration-or-remaining label. `i` is the chapter's index in the list.
/// The duration shows real book-time — matching bookmark stamps and the
/// book detail page (#2344) — while the current row's "remaining" is a
/// rate-adjusted estimate of the listening time left.
fn chapter_row(
    i: usize,
    ch: &ChapterInfo,
    current_chapter_index: usize,
    elapsed: f64,
    rate: f64,
    on_seek: EventHandler<f64>,
) -> Element {
    let is_played = i < current_chapter_index;
    let is_current = i == current_chapter_index;
    let row_class = if is_current {
        "lp-drawer-row current"
    } else if is_played {
        "lp-drawer-row played"
    } else {
        "lp-drawer-row"
    };
    let row_testid = if is_current {
        "chapter-row-current"
    } else if is_played {
        "chapter-row-played"
    } else {
        "chapter-row-upcoming"
    };

    let dur_label = format_hms(ch.duration_seconds);
    let remaining_in_ch = if is_current {
        let r = (ch.start_seconds + ch.duration_seconds - elapsed).max(0.0);
        Some(format!(
            "{} remaining",
            format_hms(remaining_at_rate(r, rate))
        ))
    } else {
        None
    };

    let ch_start = ch.start_seconds;
    let title = if ch.title.is_empty() {
        format!("Chapter {}", i + 1)
    } else {
        ch.title.clone()
    };
    let ordinal = i + 1;

    rsx! {
        button {
            key: "{ch.ordinal}",
            class: "{row_class}",
            "data-testid": "{row_testid}",
            r#type: "button",
            onclick: move |_| on_seek.call(ch_start),
            span { class: "lp-drawer-ord",
                if is_played {
                    "\u{2713}"
                } else if is_current {
                    "\u{25b6}"
                } else {
                    "{ordinal}"
                }
            }
            span { class: "lp-drawer-title", "{title}" }
            span { class: "lp-drawer-dur",
                if let Some(rem) = remaining_in_ch {
                    "{rem}"
                } else {
                    "{dur_label}"
                }
            }
        }
    }
}

#[component]
pub(super) fn ChaptersDrawer(
    chapters: Vec<ChapterInfo>,
    current_chapter_index: usize,
    elapsed: f64,
    /// Current playback rate — only the current chapter's "remaining" label
    /// divides by it; chapter durations show real book-time (#2344).
    rate: f64,
    on_seek: EventHandler<f64>,
    on_close: EventHandler<()>,
) -> Element {
    rsx! {
        ListenDrawerShell {
            testid: "chapters-drawer",
            on_close,
            head: rsx! {
                div {
                    div { class: "lp-panel-kicker", "Chapters" }
                    div { class: "lp-panel-title", "Contents" }
                }
            },
            if chapters.is_empty() {
                div { class: "lp-drawer-empty",
                    p { class: "lp-drawer-empty-title", "No chapters" }
                    p { class: "lp-drawer-empty-detail",
                        "This audiobook has no embedded chapter markers."
                    }
                }
            } else {
                for (i, ch) in chapters.iter().enumerate() {
                    {chapter_row(i, ch, current_chapter_index, elapsed, rate, on_seek)}
                }
            }
        }
    }
}

// Render-smoke coverage of the drawer's one-clock contract — a separate
// module because SSR (`dioxus::ssr`) needs the `server` feature, and the
// harnesses run inside a real `VirtualDom` because `EventHandler::new` needs
// a live runtime.
#[cfg(all(test, feature = "server"))]
mod render_tests {
    use super::*;
    use crate::test_support::render_in_vdom;

    /// Two 30-minute chapters, 20 book-minutes into the first.
    fn drawer(rate: f64) -> Element {
        rsx! {
            ChaptersDrawer {
                chapters: vec![
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
                ],
                current_chapter_index: 0,
                elapsed: 1200.0,
                rate,
                on_seek: EventHandler::new(|_: f64| {}),
                on_close: EventHandler::new(|()| {}),
            }
        }
    }

    fn at_1x() -> Element {
        drawer(1.0)
    }

    fn at_2x() -> Element {
        drawer(2.0)
    }

    #[test]
    fn chapter_rows_read_book_time_at_1x() {
        let html = render_in_vdom(at_1x);
        // Current chapter: 10:00 of book time left. Upcoming: its full 30:00.
        assert!(html.contains("10:00 remaining"), "{html}");
        assert!(html.contains("30:00"), "{html}");
    }

    // Regression for issue #2344: chapter durations show real book-time at any
    // speed (matching bookmark stamps + the detail page), while only the
    // current chapter's "remaining" is rate-adjusted.
    #[test]
    fn chapter_rows_keep_book_time_durations_but_scale_the_remaining() {
        let html = render_in_vdom(at_2x);
        // The upcoming chapter's 30:00 duration is unchanged from 1x book-time...
        assert!(html.contains("30:00"), "{html}");
        // ...while the current chapter's remaining halves at 2x.
        assert!(html.contains("5:00 remaining"), "{html}");
        // No rate-scaled duration (15:00) appears.
        assert!(!html.contains("15:00"), "{html}");
    }
}
