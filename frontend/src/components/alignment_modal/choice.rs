//! The multi-file declaration: sequence vs narrations, the ▲▼ re-order
//! controls, and the primary picker. Shown only when a book carries more
//! than one audio file — the mapping never guesses an order.

use dioxus::prelude::*;
use omnibus_shared::{AlignmentAudioFile, CrossFormatLinkMode};

use super::copy::fmt_hm;
use super::lanes::basename;

/// Class helper kept out of the rsx attribute position — an inline
/// if/else there trips clippy's suspicious-else-formatting lint.
fn radio_class(picked: bool) -> &'static str {
    if picked {
        "al-radio is-picked"
    } else {
        "al-radio"
    }
}

fn file_row_class(dimmed: bool) -> &'static str {
    if dimmed {
        "al-file-row al-file-row-dim"
    } else {
        "al-file-row"
    }
}

/// The required declaration when several audio files exist: sequence vs
/// narrations, the ▲▼ re-order controls (sequence), and the primary
/// picker (narrations, non-primary rows dimmed).
pub(super) fn render_choice(
    files: Vec<AlignmentAudioFile>,
    mut mode: Signal<CrossFormatLinkMode>,
    mut primary: Signal<Option<i64>>,
    mut order: Signal<Vec<i64>>,
) -> Element {
    let seq = mode() == CrossFormatLinkMode::Sequence;
    rsx! {
        div { class: "al-choice", "data-testid": "alignment-choice",
            p { class: "al-lane-label", "This book has {files.len()} audio files — what are they?" }
            div { class: "al-choice-cards",
                button {
                    class: if seq { "al-choice-card is-picked" } else { "al-choice-card" },
                    "data-testid": "alignment-mode-sequence",
                    aria_pressed: seq,
                    onclick: move |_| mode.set(CrossFormatLinkMode::Sequence),
                    strong { "One book, in sequence" }
                    span { "The files play end to end as a single audiobook." }
                }
                button {
                    class: if !seq { "al-choice-card is-picked" } else { "al-choice-card" },
                    "data-testid": "alignment-mode-narrations",
                    aria_pressed: !seq,
                    onclick: move |_| mode.set(CrossFormatLinkMode::Narrations),
                    strong { "Different narrations of the same book" }
                    span { "Each file is the complete book — pick a primary to align." }
                }
            }
            div { class: "al-file-rows",
                for (i, id) in order().iter().copied().enumerate() {
                    if let Some(f) = files.iter().find(|f| f.book_file_id == id) {
                        div { class: file_row_class(!seq && primary() != Some(id)),
                            if seq {
                                span { class: "al-file-ord", "{i + 1}" }
                                button {
                                    class: "btn ghost sm al-file-move",
                                    "data-testid": "alignment-move-up-{id}",
                                    aria_label: "Move earlier",
                                    disabled: i == 0,
                                    onclick: move |_| {
                                        let mut o = order();
                                        if i > 0 {
                                            o.swap(i, i - 1);
                                            order.set(o);
                                        }
                                    },
                                    "\u{25b2}"
                                }
                                button {
                                    class: "btn ghost sm al-file-move",
                                    "data-testid": "alignment-move-down-{id}",
                                    aria_label: "Move later",
                                    disabled: i + 1 == order().len(),
                                    onclick: move |_| {
                                        let mut o = order();
                                        if i + 1 < o.len() {
                                            o.swap(i, i + 1);
                                            order.set(o);
                                        }
                                    },
                                    "\u{25bc}"
                                }
                            } else {
                                button {
                                    class: radio_class(primary() == Some(id)),
                                    "data-testid": "alignment-primary-{id}",
                                    aria_pressed: primary() == Some(id),
                                    aria_label: "Set as primary narration",
                                    onclick: move |_| primary.set(Some(id)),
                                }
                            }
                            span { class: "al-file-name", title: "{f.label}", "{basename(&f.label)}" }
                            if !seq && primary() == Some(id) {
                                span { class: "al-primary-pill", "primary — aligned with the ebook" }
                            }
                            span { class: "al-seg-dur", "{fmt_hm(f.duration_seconds)}" }
                        }
                    }
                }
            }
            if !seq {
                p { class: "al-narr-note",
                    "The other narration stays available from the Listen menu — it just "
                    "isn't the one Omnibus keeps in step."
                }
            }
        }
    }
}
