//! Cross-format alignment modal: the activation surface for position sync.
//! Shows the text and audio lanes with chapter ticks and both current
//! positions, the sequence-vs-narrations declaration for multi-file audio,
//! and the linear mapped preview — sync turns on only when the user
//! confirms here. Mirrors `MergeDialog`'s shell: busy-guarded dismissal,
//! scrolling body, pinned foot, `role="alert"` error banner.

use dioxus::prelude::*;
use omnibus_shared::{
    AlignmentAudioFile, AlignmentView, ConfirmCrossFormatLink, CrossFormatLinkMode,
};

use crate::components::confirm_modal::ConfirmModal;
use crate::data;

/// `"6h 31m"` / `"41m"` for lane labels and the mapped preview.
pub(crate) fn fmt_hm(seconds: f64) -> String {
    let mins = (seconds.max(0.0) / 60.0).round() as i64;
    let (h, m) = (mins / 60, mins % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

/// The audio files in the user's working order (falls back to view order
/// for ids the order list doesn't know — a defensive no-op in practice).
fn ordered<'v>(view: &'v AlignmentView, order: &[i64]) -> Vec<&'v AlignmentAudioFile> {
    let mut files: Vec<&AlignmentAudioFile> = Vec::with_capacity(view.audio_files.len());
    for id in order {
        if let Some(f) = view.audio_files.iter().find(|f| f.book_file_id == *id) {
            files.push(f);
        }
    }
    for f in &view.audio_files {
        if !order.contains(&f.book_file_id) {
            files.push(f);
        }
    }
    files
}

/// Whole-timeline percent of the current listening position, given the
/// working order; `None` when the position's file isn't on the timeline.
fn listening_pct(view: &AlignmentView, files: &[&AlignmentAudioFile]) -> Option<f64> {
    let pos = view.listening.as_ref()?;
    let total: f64 = files.iter().map(|f| f.duration_seconds).sum();
    if total <= 0.0 {
        return None;
    }
    // A file-less position is only unambiguous on a single-file timeline —
    // the same refusal the mapping engine applies.
    let file_id = pos
        .book_file_id
        .or_else(|| (files.len() == 1).then(|| files[0].book_file_id))?;
    let mut start = 0.0;
    for f in files {
        if f.book_file_id == file_id {
            let global = start + pos.seconds.clamp(0.0, f.duration_seconds);
            return Some((100.0 * global / total).clamp(0.0, 100.0));
        }
        start += f.duration_seconds;
    }
    None
}

/// The alignment modal. Fetches its view on open; `on_changed` fires after
/// a confirmed link or unlink so the parent can refresh its entry row.
#[component]
pub fn AlignmentModal(uuid: String, open: Signal<bool>, on_changed: EventHandler<()>) -> Element {
    let mut view = use_signal(|| None::<AlignmentView>);
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

    rsx! {
        ConfirmModal {
            testid: "alignment-modal",
            aria_label: "Cross-format sync alignment",
            dialog_class: "al-modal card",
            busy: busy(),
            on_dismiss: move |_| open.set(false),
            div { class: "al-head",
                p { class: "al-kicker", "Cross-format sync" }
                h3 {
                    if multi {
                        "Several audio files — what are they?"
                    } else {
                        "Do these line up?"
                    }
                }
                p { class: "al-sub",
                    "Check that the mapped position lands about where it should, then confirm. "
                    "Sync stays off until you do, and you can unlink any time."
                }
            }
            if let Some(v) = view() {
                {render_lanes(&v, &order(), mode(), primary())}
                if multi {
                    {render_choice(v.audio_files.clone(), mode, primary, order)}
                }
                if v.ebook.is_none() {
                    p { class: "al-lowconf", role: "note", "data-testid": "alignment-lowconf",
                        "No chapter anchors yet — this mapping is a straight percent-for-percent "
                        "estimate, so jumps land close, not exact."
                    }
                }
            } else if error().is_none() {
                p { class: "al-loading", "Measuring both formats…" }
            }
            if let Some(e) = error() {
                p { role: "alert", class: "bd-merge-error", "{e}" }
            }
            div { class: "al-foot",
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
                    class: "btn ghost sm",
                    "data-testid": "alignment-cancel",
                    disabled: busy(),
                    onclick: move |_| open.set(false),
                    "Cancel"
                }
                button {
                    class: "btn sm",
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
                    "Looks right — turn on sync"
                }
            }
        }
    }
}

/// Both lanes plus markers: text lane with chapter ticks and the reading
/// marker, audio lane segments with the listening marker and the dashed
/// linear mapped-preview marker (same global percent as the reading spot).
fn render_lanes(
    view: &AlignmentView,
    order: &[i64],
    mode: CrossFormatLinkMode,
    primary: Option<i64>,
) -> Element {
    let files_all = ordered(view, order);
    let files: Vec<&AlignmentAudioFile> = match mode {
        CrossFormatLinkMode::Sequence => files_all,
        CrossFormatLinkMode::Narrations => files_all
            .into_iter()
            .filter(|f| Some(f.book_file_id) == primary)
            .collect(),
    };
    let total: f64 = files.iter().map(|f| f.duration_seconds).sum();
    let reading_pct = view.reading.as_ref().and_then(|r| r.percent);
    let listen_pct = listening_pct(view, &files);
    let ebook_chapters = view
        .ebook
        .as_ref()
        .map(|e| e.chapters.clone())
        .unwrap_or_default();

    rsx! {
        div { class: "al-lanes", "data-testid": "alignment-lanes",
            p { class: "al-lane-label",
                "Ebook · percent of text"
                if !ebook_chapters.is_empty() {
                    " · {ebook_chapters.len()} chapters"
                }
            }
            div { class: "al-lane al-lane-text",
                for ch in ebook_chapters.iter() {
                    div {
                        class: "al-tick",
                        title: "{ch.title}",
                        style: "left: {ch.percent}%",
                    }
                }
                if let Some(pct) = reading_pct {
                    div {
                        class: "al-marker",
                        "data-testid": "alignment-reading-marker",
                        style: "left: {pct}%",
                    }
                }
            }
            p { class: "al-lane-label",
                "Audiobook · "
                if files.len() == 1 {
                    "{fmt_hm(total)}"
                } else {
                    "{files.len()} files, played end to end · {fmt_hm(total)}"
                }
            }
            div { class: "al-lane al-lane-audio",
                for f in files.iter() {
                    div {
                        class: "al-seg",
                        style: format!(
                            "flex-grow: {}",
                            if total > 0.0 { f.duration_seconds.max(1.0) } else { 1.0 },
                        ),
                        span { class: "al-seg-label", "{f.label}" }
                        span { class: "al-seg-dur", "{fmt_hm(f.duration_seconds)}" }
                    }
                }
                if let Some(pct) = listen_pct {
                    div {
                        class: "al-marker",
                        "data-testid": "alignment-listening-marker",
                        style: "left: {pct}%",
                    }
                }
                if let Some(pct) = reading_pct {
                    div {
                        class: "al-marker al-marker-mapped",
                        "data-testid": "alignment-mapped-marker",
                        style: "left: {pct}%",
                    }
                }
            }
            if let (Some(pct), true) = (reading_pct, total > 0.0) {
                p { class: "al-mapped-note",
                    "≈ your reading maps to {fmt_hm(total * pct as f64 / 100.0)}"
                }
            }
        }
    }
}

/// Class helper kept out of the rsx attribute position — an inline
/// if/else there trips clippy's suspicious-else-formatting lint.
fn radio_class(picked: bool) -> &'static str {
    if picked {
        "al-radio is-picked"
    } else {
        "al-radio"
    }
}

/// The required declaration when several audio files exist: sequence vs
/// narrations, the ▲▼ re-order controls (sequence), and the primary
/// picker (narrations).
fn render_choice(
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
                    onclick: move |_| mode.set(CrossFormatLinkMode::Sequence),
                    strong { "One book, in sequence" }
                    span { "The files play end to end as a single audiobook." }
                }
                button {
                    class: if !seq { "al-choice-card is-picked" } else { "al-choice-card" },
                    "data-testid": "alignment-mode-narrations",
                    onclick: move |_| mode.set(CrossFormatLinkMode::Narrations),
                    strong { "Different narrations of the same book" }
                    span { "Each file is the complete book — pick a primary to align." }
                }
            }
            div { class: "al-file-rows",
                for (i, id) in order().iter().copied().enumerate() {
                    if let Some(f) = files.iter().find(|f| f.book_file_id == id) {
                        div { class: "al-file-row",
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
                                    aria_label: "Set as primary narration",
                                    onclick: move |_| primary.set(Some(id)),
                                }
                            }
                            span { class: "al-file-name", "{f.label}" }
                            span { class: "al-seg-dur", "{fmt_hm(f.duration_seconds)}" }
                        }
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fmt_hm;

    #[test]
    fn fmt_hm_renders_hours_and_bare_minutes() {
        assert_eq!(fmt_hm(23_460.0), "6h 31m");
        assert_eq!(fmt_hm(2_460.0), "41m");
        assert_eq!(fmt_hm(0.0), "0m");
        assert_eq!(fmt_hm(-5.0), "0m");
    }
}
