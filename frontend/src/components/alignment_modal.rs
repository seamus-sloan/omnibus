//! Cross-format alignment modal: the activation surface for position sync.
//! Carries its own book identity (it also opens from the merge dialog),
//! draws both lanes with chapter ticks, position chips, and the connector
//! field whose dashed thread is the "do these line up?" judgement, and
//! previews the mapped position through the same anchor pairs the jump
//! uses. Sync turns on only when the user confirms here. Mirrors
//! `MergeDialog`'s shell: busy-guarded dismissal, `role="alert"` errors.
//! The lanes, the multi-file choice, and the wording live in the
//! sub-modules below; this file is the shell around them.

use dioxus::prelude::*;
use omnibus_shared::{AlignmentView, ConfirmCrossFormatLink, CrossFormatLinkMode, EbookMetadata};

use crate::components::confirm_modal::ConfirmModal;
use crate::components::sync_glyph::SyncGlyph;
use crate::data;

mod choice;
mod copy;
mod lanes;

pub(crate) use copy::{fmt_hm, recency};
pub(crate) use lanes::{interpolate_pairs, listening_frac};

use choice::render_choice;
use copy::{
    align_mode, confirm_label, ebook_chapter_count, foot_note, linear_pill, sub_header, SUB_DEFAULT,
};
use lanes::render_lanes;

/// The alignment modal. Fetches its view (and the book's identity) on
/// open; `on_changed` fires after a confirmed link or unlink so the parent
/// can refresh its entry row.
#[component]
pub fn AlignmentModal(uuid: String, open: Signal<bool>, on_changed: EventHandler<()>) -> Element {
    let mut view = use_signal(|| None::<AlignmentView>);
    let mut book = use_signal(|| None::<EbookMetadata>);
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    let mut mode = use_signal(|| CrossFormatLinkMode::Sequence);
    let mut primary = use_signal(|| None::<i64>);
    let mut order = use_signal(Vec::<i64>::new);

    let fetch_uuid = uuid.clone();
    use_effect(move || {
        if !open() {
            return;
        }
        let uuid = fetch_uuid.clone();
        view.set(None);
        error.set(None);
        spawn(async move {
            // Identity is best-effort chrome; the alignment payload is the
            // load-bearing fetch.
            if let Ok(b) = data::get_ebook("", &uuid).await {
                book.set(b);
            }
            match data::get_alignment("", &uuid).await {
                Ok(v) => {
                    let stored_mode = v
                        .link
                        .as_ref()
                        .map(|l| l.mode)
                        .unwrap_or(CrossFormatLinkMode::Sequence);
                    let stored_primary = v
                        .link
                        .as_ref()
                        .and_then(|l| l.primary_book_file_id)
                        .or_else(|| v.audio_files.first().map(|f| f.book_file_id));
                    mode.set(stored_mode);
                    primary.set(stored_primary);
                    order.set(v.audio_files.iter().map(|f| f.book_file_id).collect());
                    view.set(Some(v));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });

    if !open() {
        return rsx! {};
    }

    let confirm_uuid = uuid.clone();
    let has_link = view().as_ref().is_some_and(|v| v.link.is_some());
    let multi = view().as_ref().is_some_and(|v| v.audio_files.len() > 1);
    let align = view().as_ref().map(align_mode);
    let reordered = view().as_ref().is_some_and(|v| {
        order()
            != v.audio_files
                .iter()
                .map(|f| f.book_file_id)
                .collect::<Vec<_>>()
    });
    let save_order = reordered && mode() == CrossFormatLinkMode::Sequence;
    // While the view is loading the confirm button is disabled, so its
    // label and the footer only need a sane default.
    let confirm_text = align.map_or("Looks right — turn on sync", |a| {
        confirm_label(a, save_order)
    });
    let foot = align.map_or(
        Some("Sync stays off until you confirm. You can unlink any time."),
        foot_note,
    );
    let sub_text = view().as_ref().map_or_else(
        || SUB_DEFAULT.to_string(),
        |v| sub_header(align_mode(v), v.audio_chapter_marks, ebook_chapter_count(v)),
    );

    // The design themes the whole modal off the BOOK's accent — fills,
    // chips, markers, and the header wash all derive from it. Absent an
    // extracted accent the app default cascades through unchanged.
    let accent_style = book()
        .as_ref()
        .and_then(|b| b.accent.clone())
        .map(|a| format!("--accent: {a};"))
        .unwrap_or_default();
    rsx! {
        ConfirmModal {
            testid: "alignment-modal",
            aria_label: "Cross-format sync alignment",
            dialog_class: "al-modal card",
            busy: busy(),
            on_dismiss: move |_| open.set(false),
            div { class: "al-body", style: "{accent_style}",
            div { class: "al-head",
                p { class: "al-kicker",
                    span { class: "al-kicker-glyph", SyncGlyph { size: 13 } }
                    "Cross-format sync"
                }
                h3 { "Link formats" }
                {render_identity(&book())}
                p { class: "al-sub", "{sub_text}" }
            }
            if let Some(v) = view() {
                {render_lanes(&v, &order(), mode(), primary())}
                if multi {
                    {render_choice(v.audio_files.clone(), mode, primary, order)}
                }
                if v.link.as_ref().is_some_and(|l| l.stale) {
                    // Anchoring (and the marks count) is suppressed while
                    // stale — say so instead of misreading the zeros.
                    p { class: "al-lowconf", role: "note", "data-testid": "alignment-lowconf",
                        "The audio files changed since you linked — sync is paused, and "
                        "the alignment re-checks when you confirm again."
                    }
                } else if let Some(m) = v.anchor_match {
                    p { class: "al-matched", role: "note", "data-testid": "alignment-match",
                        "\u{2713} {m.matched} of {m.ebook_chapters} chapters matched — "
                        "jumps land chapter-accurately."
                    }
                } else {
                    p { class: "al-matched al-matched-warn", role: "note", "data-testid": "alignment-linear-pill",
                        "~ {linear_pill(v.audio_chapter_marks, ebook_chapter_count(&v))}"
                    }
                    div { class: "al-lowconf-callout", role: "note", "data-testid": "alignment-lowconf",
                        span { class: "al-warn-badge", "!" }
                        if v.audio_chapter_marks > 0 {
                            div {
                                strong { "The chapter counts don\u{2019}t match, so sync will go by percentage instead. " }
                                "Jumps will land close, but not exactly. Extra marks are usually "
                                "opening notes, bonus scenes, credits, remarks, or part numbers."
                            }
                        } else {
                            div {
                                strong { "This mapping is a straight percent-for-percent estimate. " }
                                "Jumps will land close, not exact — expect drift of several "
                                "minutes, especially mid-book. Adding chapter marks to the audio "
                                "file later will tighten it up."
                            }
                        }
                    }
                }
            } else if error().is_none() {
                p { class: "al-loading", "Measuring both formats…" }
            }
            if let Some(e) = error() {
                p { role: "alert", class: "bd-merge-error", "{e}" }
            }
            div { class: "al-foot",
                if let Some(note) = foot {
                    span { class: "al-foot-note", "{note}" }
                }
                if has_link {
                    button {
                        class: "btn ghost sm",
                        "data-testid": "alignment-unlink",
                        disabled: busy(),
                        onclick: {
                            let uuid = uuid.clone();
                            move |_| {
                                let uuid = uuid.clone();
                                busy.set(true);
                                error.set(None);
                                spawn(async move {
                                    match data::unlink_cross_format("", &uuid).await {
                                        Ok(_) => {
                                            busy.set(false);
                                            open.set(false);
                                            on_changed.call(());
                                        }
                                        Err(e) => {
                                            busy.set(false);
                                            error.set(Some(e.to_string()));
                                        }
                                    }
                                });
                            }
                        },
                        "Unlink"
                    }
                }
                span { class: "al-foot-spacer" }
                button {
                    class: "btn ghost",
                    "data-testid": "alignment-cancel",
                    disabled: busy(),
                    onclick: move |_| open.set(false),
                    "Cancel"
                }
                // Full-size accent primary per the design.
                button {
                    class: "btn primary",
                    "data-testid": "alignment-confirm",
                    disabled: busy() || view().is_none(),
                    onclick: move |_| {
                        let Some(v) = view() else { return };
                        let original: Vec<i64> =
                            v.audio_files.iter().map(|f| f.book_file_id).collect();
                        let reordered = order() != original;
                        let update = ConfirmCrossFormatLink {
                            book_uuid: confirm_uuid.clone(),
                            mode: mode(),
                            primary_book_file_id: if mode() == CrossFormatLinkMode::Narrations {
                                primary()
                            } else {
                                None
                            },
                            audio_order: if reordered && mode() == CrossFormatLinkMode::Sequence {
                                Some(order())
                            } else {
                                None
                            },
                        };
                        busy.set(true);
                        error.set(None);
                        spawn(async move {
                            match data::confirm_cross_format_link("", update).await {
                                Ok(()) => {
                                    busy.set(false);
                                    open.set(false);
                                    on_changed.call(());
                                }
                                Err(e) => {
                                    busy.set(false);
                                    error.set(Some(e.to_string()));
                                }
                            }
                        });
                    },
                    "{confirm_text}"
                }
            }
            }
        }
    }
}

/// The book identity row: the modal opens from the merge dialog too, so it
/// must say which book it is about without referencing the page behind it.
fn render_identity(book: &Option<EbookMetadata>) -> Element {
    let Some(b) = book else {
        return rsx! {};
    };
    let title = b.title.clone().unwrap_or_else(|| b.filename.clone());
    let author = b
        .creators
        .first()
        .map(|c| c.name.clone())
        .unwrap_or_default();
    rsx! {
        div { class: "al-identity", "data-testid": "alignment-identity",
            span { class: "al-identity-title", "{title}" }
            if !author.is_empty() {
                span { class: "al-identity-author", "{author}" }
            }
            span { class: "al-identity-spacer" }
            span { class: "al-pair-mark", SyncGlyph { size: 13 } }
        }
    }
}
