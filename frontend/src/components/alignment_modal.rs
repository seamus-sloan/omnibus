//! Cross-format alignment modal: the activation surface for position sync.
//! Carries its own book identity (it also opens from the merge dialog),
//! draws both lanes with chapter ticks, position chips, and the connector
//! field whose dashed thread is the "do these line up?" judgement, and
//! previews the mapped position through the same anchor pairs the jump
//! uses. Sync turns on only when the user confirms here. Mirrors
//! `MergeDialog`'s shell: busy-guarded dismissal, `role="alert"` errors.

use dioxus::prelude::*;
use omnibus_shared::{
    AlignmentAudioFile, AlignmentView, ConfirmCrossFormatLink, CrossFormatLinkMode, EbookMetadata,
};

use crate::components::confirm_modal::ConfirmModal;
use crate::components::sync_glyph::SyncGlyph;
use crate::data;

/// `"6h 31m"` / `"41m"` for lane labels and the mapped preview. Floor
/// minutes — a duration label must never claim more time than exists.
pub(crate) fn fmt_hm(seconds: f64) -> String {
    let mins = (seconds.max(0.0) / 60.0).floor() as i64;
    let (h, m) = (mins / 60, mins % 60);
    if h > 0 {
        format!("{h}h {m:02}m")
    } else {
        format!("{m}m")
    }
}

/// Coarse recency for sync copy ("yesterday"), from a client event clock.
/// These surfaces render post-mount only, so SSR never sees the value
/// (rule 07 holds) and `crate::time::now_unix` serves every target.
#[cfg_attr(feature = "mobile", allow(dead_code))]
pub(crate) fn recency(clock_epoch_secs: i64) -> String {
    let d = (crate::time::now_unix() - clock_epoch_secs).max(0);
    if d < 90 {
        "just now".into()
    } else if d < 3_600 {
        format!("{}m ago", d / 60)
    } else if d < 86_400 {
        format!("{}h ago", d / 3_600)
    } else if d < 172_800 {
        "yesterday".into()
    } else {
        format!("{}d ago", d / 86_400)
    }
}

/// Piecewise-linear map of a text fraction through the served anchor pairs
/// (implicit `(0,0)`/`(1,1)` endpoints) — the same interpolation the jump
/// runs server-side, so the preview cannot disagree with it.
fn interpolate_pairs(pairs: &[(f64, f64)], frac: f64) -> f64 {
    let frac = frac.clamp(0.0, 1.0);
    let mut prev = (0.0f64, 0.0f64);
    for (t, a) in pairs.iter().copied().chain(std::iter::once((1.0, 1.0))) {
        if frac <= t {
            if (t - prev.0).abs() < f64::EPSILON {
                return prev.1;
            }
            let k = (frac - prev.0) / (t - prev.0);
            return (prev.1 + k * (a - prev.1)).clamp(0.0, 1.0);
        }
        prev = (t, a);
    }
    frac
}

/// Which notice/confirm mode the modal is in, derived from the served
/// view. Stale wins — anchoring and the marks count are suppressed while
/// a link is stale, so their zeros must not be read as a linear mode.
#[derive(Debug, Clone, Copy, PartialEq)]
enum AlignMode {
    /// The audio set drifted since confirmation; re-confirm re-checks.
    Stale,
    /// Chapter anchoring engaged — jumps land chapter-accurately.
    Anchored,
    /// Marks exist but can't be paired 1:1 — percent mapping.
    Mismatch,
    /// No usable marks at all — percent mapping.
    NoMarks,
}

fn align_mode(view: &AlignmentView) -> AlignMode {
    if view.link.as_ref().is_some_and(|l| l.stale) {
        AlignMode::Stale
    } else if view.anchor_match.is_some() {
        AlignMode::Anchored
    } else if view.audio_chapter_marks > 0 {
        AlignMode::Mismatch
    } else {
        AlignMode::NoMarks
    }
}

/// The ebook chapter count the mismatch copy quotes; `None` while the
/// structure backfill hasn't reached the book (no lane ticks either).
fn ebook_chapter_count(view: &AlignmentView) -> Option<usize> {
    view.ebook
        .as_ref()
        .map(|e| e.chapters.len())
        .filter(|n| *n > 0)
}

/// The default judgement prompt under the title.
const SUB_DEFAULT: &str =
    "Check that the mapped position lands about where it should, then confirm.";

/// Singular/plural noun for a count, so one mark never reads "1 marks".
fn plural(n: i64, one: &str, many: &str) -> String {
    if n == 1 {
        format!("{n} {one}")
    } else {
        format!("{n} {many}")
    }
}

/// The intro line under the title: the default prompt, or the honest
/// explanation of why the mapping is percentage-based.
fn sub_header(mode: AlignMode, marks: i64, ebook_chapters: Option<usize>) -> String {
    let marks_n = plural(marks, "chapter mark", "chapter marks");
    match mode {
        AlignMode::Stale | AlignMode::Anchored => SUB_DEFAULT.into(),
        AlignMode::NoMarks => "This audiobook carries no chapter markers, so there\u{2019}s \
             nothing to anchor the mapping to."
            .into(),
        AlignMode::Mismatch => match ebook_chapters {
            Some(m) => {
                let chapters_n = plural(m as i64, "chapter", "chapters");
                format!(
                    "The audio carries {marks_n} but the book has {chapters_n}, \
                     so the chapters can\u{2019}t be paired up exactly."
                )
            }
            None => {
                let pronoun = if marks == 1 { "it" } else { "they" };
                format!(
                    "The audio carries {marks_n}, but {pronoun} can\u{2019}t be \
                     paired up with the book\u{2019}s chapters exactly."
                )
            }
        },
    }
}

/// The `~` warn pill's text for the two percentage modes. Without an
/// ebook chapter count the mismatch line drops the vs-clause rather than
/// quoting a number it doesn't have.
fn linear_pill(marks: i64, ebook_chapters: Option<usize>) -> String {
    if marks == 0 {
        "no chapter marks in the audio — linear estimate".into()
    } else {
        let marks_n = plural(marks, "audio mark", "audio marks");
        match ebook_chapters {
            Some(m) => {
                let chapters_n = plural(m as i64, "chapter", "chapters");
                format!("{marks_n} vs {chapters_n} — no 1:1 match")
            }
            None => format!("{marks_n} — no 1:1 match"),
        }
    }
}

/// The fresh-confirm button names what confirming turns on; a pending
/// sequence reorder rides along as a prefix. The stale re-confirm keeps
/// its original labels — anchoring is suppressed there, so neither mode
/// name would be honest.
fn confirm_label(mode: AlignMode, save_order: bool) -> &'static str {
    match (mode, save_order) {
        (AlignMode::Stale, false) => "Looks right — turn on sync",
        (AlignMode::Stale, true) => "Save order & turn on sync",
        (AlignMode::Anchored, false) => "Sync Based On Chapters",
        (AlignMode::Anchored, true) => "Save order — Sync Based On Chapters",
        (AlignMode::Mismatch | AlignMode::NoMarks, false) => "Sync Based Off Percentage",
        (AlignMode::Mismatch | AlignMode::NoMarks, true) => {
            "Save order — Sync Based Off Percentage"
        }
    }
}

/// The mono footer line; the percentage modes omit it per the design —
/// the warn banner already carries the caveat.
fn foot_note(mode: AlignMode) -> Option<&'static str> {
    match mode {
        AlignMode::Stale | AlignMode::Anchored => {
            Some("Sync stays off until you confirm. You can unlink any time.")
        }
        AlignMode::Mismatch | AlignMode::NoMarks => None,
    }
}

/// The distinguishing tail of a library-relative label: real libraries
/// nest under `Author/Book/…`, so the shared prefix carries nothing and
/// ellipsis truncation would hide the part that differs.
fn basename(label: &str) -> &str {
    label.rsplit('/').next().unwrap_or(label)
}

/// Thinning step so a 196-chapter nav renders a legible lane instead of a
/// tick barcode.
const MAX_LANE_TICKS: usize = 48;

fn tick_step(len: usize) -> usize {
    len.div_ceil(MAX_LANE_TICKS).max(1)
}

/// Two chip centres closer than this fraction of the lane width collide —
/// stagger the mapped chip onto a second row.
const CHIP_COLLISION_FRACTION: f64 = 0.34;

/// Horizontal translate keeping a chip inside the lane at the edges.
fn chip_translate(frac: f64) -> &'static str {
    if frac < 0.09 {
        "0%"
    } else if frac > 0.91 {
        "-100%"
    } else {
        "-50%"
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

/// Whole-timeline fraction of the current listening position, given the
/// working order; `None` when the position's file isn't on the timeline.
fn listening_frac(view: &AlignmentView, files: &[&AlignmentAudioFile]) -> Option<f64> {
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
            return Some((global / total).clamp(0.0, 1.0));
        }
        start += f.duration_seconds;
    }
    None
}

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
                h3 {
                    if multi {
                        "Several audio files — what are they?"
                    } else {
                        "Do these line up?"
                    }
                }
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

/// Both lanes plus the connector field between them: chapter ticks, fills,
/// labelled position chips, the anchored mapped preview, and the delta
/// sentence beneath.
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
    let reading_frac = view
        .reading
        .as_ref()
        .and_then(|r| r.percent)
        .map(|p| (p as f64 / 100.0).clamp(0.0, 1.0));
    let listen_frac = listening_frac(view, &files);
    // The preview maps through the same anchor pairs the jump uses; with
    // no pairs it is the honest linear identity.
    let mapped_frac = reading_frac.map(|f| interpolate_pairs(&view.anchor_pairs, f));
    let ebook_chapters = view
        .ebook
        .as_ref()
        .map(|e| e.chapters.clone())
        .unwrap_or_default();
    let text_step = tick_step(ebook_chapters.len());
    // Audio ticks: per-file chapter starts as global timeline fractions.
    let mut audio_ticks: Vec<f64> = Vec::new();
    if total > 0.0 {
        let mut start = 0.0f64;
        for f in &files {
            for s in &f.chapter_starts {
                audio_ticks.push(((start + s) / total).clamp(0.0, 1.0));
            }
            start += f.duration_seconds;
        }
    }
    let audio_step = tick_step(audio_ticks.len());
    let stagger = match (listen_frac, mapped_frac) {
        (Some(l), Some(m)) => (l - m).abs() < CHIP_COLLISION_FRACTION,
        _ => false,
    };
    let delta = match (listen_frac, mapped_frac, total > 0.0) {
        (Some(l), Some(m), true) => Some((m - l) * total),
        _ => None,
    };
    let no_positions = reading_frac.is_none() && listen_frac.is_none();

    rsx! {
        div { class: "al-lanes", "data-testid": "alignment-lanes",
            p { class: "al-lane-label",
                "Ebook · percent of text"
                if !ebook_chapters.is_empty() {
                    " · {ebook_chapters.len()} chapters"
                }
            }
            if let Some(f) = reading_frac {
                div { class: "al-chip-row",
                    span {
                        class: "al-chip al-chip-accent",
                        style: "left: {f * 100.0}%; transform: translateX({chip_translate(f)});",
                        "Reading · {view.reading.as_ref().and_then(|r| r.percent).unwrap_or(0)}%"
                    }
                }
            }
            div { class: "al-lane al-lane-text",
                if let Some(f) = reading_frac {
                    div { class: "al-fill", style: "width: {f * 100.0}%" }
                }
                for (i, ch) in ebook_chapters.iter().enumerate() {
                    if i % text_step == 0 {
                        div {
                            class: "al-tick",
                            title: "{ch.title}",
                            style: "left: {ch.percent}%",
                        }
                    }
                }
                if let Some(f) = reading_frac {
                    div {
                        class: "al-marker",
                        "data-testid": "alignment-reading-marker",
                        style: "left: {f * 100.0}%",
                    }
                }
            }
            // The connector field: faint threads join matched anchors; the
            // accent dashed thread is the reading position landing in the
            // audio — the thing the user judges before confirming.
            div { class: "al-connector", "aria-hidden": "true",
                svg {
                    view_box: "0 0 1000 44",
                    preserve_aspect_ratio: "none",
                    for (t, a) in view.anchor_pairs.iter() {
                        line {
                            x1: "{t * 1000.0}",
                            y1: "0",
                            x2: "{a * 1000.0}",
                            y2: "44",
                            class: "al-thread",
                        }
                    }
                    if let (Some(f), Some(m)) = (reading_frac, mapped_frac) {
                        line {
                            x1: "{f * 1000.0}",
                            y1: "0",
                            x2: "{m * 1000.0}",
                            y2: "44",
                            class: "al-thread-mapped",
                        }
                    }
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
                        span { class: "al-seg-label", title: "{f.label}", "{basename(&f.label)}" }
                        span { class: "al-seg-dur", "{fmt_hm(f.duration_seconds)}" }
                    }
                }
                div { class: "al-lane-overlay",
                    if let Some(f) = listen_frac {
                        div { class: "al-fill", style: "width: {f * 100.0}%" }
                    }
                    for (i, t) in audio_ticks.iter().enumerate() {
                        if i % audio_step == 0 {
                            div { class: "al-tick", style: "left: {t * 100.0}%" }
                        }
                    }
                    if let Some(f) = listen_frac {
                        div {
                            class: "al-marker",
                            "data-testid": "alignment-listening-marker",
                            style: "left: {f * 100.0}%",
                        }
                    }
                    if let Some(m) = mapped_frac {
                        div {
                            class: "al-marker al-marker-mapped",
                            "data-testid": "alignment-mapped-marker",
                            style: "left: {m * 100.0}%",
                        }
                    }
                }
            }
            div { class: if stagger { "al-chip-row al-chip-row-tall" } else { "al-chip-row" },
                if let Some(f) = listen_frac {
                    span {
                        class: "al-chip",
                        style: "left: {f * 100.0}%; transform: translateX({chip_translate(f)});",
                        "Listening · {fmt_hm(f * total)}"
                    }
                }
                if let Some(m) = mapped_frac {
                    span {
                        class: if stagger { "al-chip al-chip-accent al-chip-dashed al-chip-staggered" } else { "al-chip al-chip-accent al-chip-dashed" },
                        style: "left: {m * 100.0}%; transform: translateX({chip_translate(m)});",
                        "≈ your reading maps here · {fmt_hm(m * total)}"
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
            if no_positions {
                p { class: "al-mapped-note", "data-testid": "alignment-empty-note",
                    "Nothing to compare yet — read or listen a little first, and the "
                    "positions will land on these lanes."
                }
            } else if let Some(d) = delta {
                p { class: "al-mapped-note",
                    if d >= 0.0 {
                        "Your reading spot sits ≈ {fmt_hm(d.abs())} past the listening position."
                    } else {
                        "The listening position is ≈ {fmt_hm(d.abs())} past your reading spot."
                    }
                }
            } else if let (Some(m), true) = (mapped_frac, total > 0.0) {
                p { class: "al-mapped-note",
                    "≈ your reading maps to {fmt_hm(m * total)}"
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

#[cfg(test)]
mod tests {
    use omnibus_shared::{
        AlignmentLink, AlignmentMatch, AlignmentView, CrossFormatLinkMode, MappingConfidence,
    };

    use super::{
        align_mode, basename, confirm_label, fmt_hm, foot_note, interpolate_pairs, linear_pill,
        sub_header, tick_step, AlignMode, SUB_DEFAULT,
    };

    fn bare_view(marks: i64) -> AlignmentView {
        AlignmentView {
            link: None,
            anchor_match: None,
            ebook: None,
            audio_files: Vec::new(),
            reading: None,
            listening: None,
            anchor_pairs: Vec::new(),
            audio_chapter_marks: marks,
        }
    }

    #[test]
    fn align_mode_splits_stale_anchored_and_the_two_linear_modes() {
        let mut v = bare_view(0);
        assert_eq!(align_mode(&v), AlignMode::NoMarks);
        v.audio_chapter_marks = 23;
        assert_eq!(align_mode(&v), AlignMode::Mismatch);
        v.anchor_match = Some(AlignmentMatch {
            matched: 12,
            ebook_chapters: 14,
            confidence: MappingConfidence::ChapterAnchored,
        });
        assert_eq!(align_mode(&v), AlignMode::Anchored);
        // Stale wins over everything — the suppressed counts must not be
        // misread as a linear mode.
        v.link = Some(AlignmentLink {
            mode: CrossFormatLinkMode::Sequence,
            primary_book_file_id: None,
            stale: true,
            confirmed_at: 0,
            follow: false,
            user_anchors: 0,
        });
        assert_eq!(align_mode(&v), AlignMode::Stale);
    }

    #[test]
    fn sub_header_explains_each_linear_mode_and_defaults_elsewhere() {
        assert_eq!(sub_header(AlignMode::Anchored, 23, Some(14)), SUB_DEFAULT);
        assert_eq!(sub_header(AlignMode::Stale, 0, None), SUB_DEFAULT);
        assert_eq!(
            sub_header(AlignMode::Mismatch, 23, Some(14)),
            "The audio carries 23 chapter marks but the book has 14 chapters, \
             so the chapters can\u{2019}t be paired up exactly."
        );
        assert_eq!(
            sub_header(AlignMode::Mismatch, 23, None),
            "The audio carries 23 chapter marks, but they can\u{2019}t be \
             paired up with the book\u{2019}s chapters exactly."
        );
        assert_eq!(
            sub_header(AlignMode::NoMarks, 0, None),
            "This audiobook carries no chapter markers, so there\u{2019}s \
             nothing to anchor the mapping to."
        );
    }

    #[test]
    fn linear_pill_quotes_both_counts_and_degrades_without_the_ebook_side() {
        assert_eq!(
            linear_pill(23, Some(14)),
            "23 audio marks vs 14 chapters — no 1:1 match"
        );
        assert_eq!(linear_pill(23, None), "23 audio marks — no 1:1 match");
        assert_eq!(
            linear_pill(0, Some(14)),
            "no chapter marks in the audio — linear estimate"
        );
        // One of anything reads singular — "1 audio marks" shipped once.
        assert_eq!(
            linear_pill(1, Some(1)),
            "1 audio mark vs 1 chapter — no 1:1 match"
        );
        assert_eq!(
            sub_header(AlignMode::Mismatch, 1, None),
            "The audio carries 1 chapter mark, but it can\u{2019}t be \
             paired up with the book\u{2019}s chapters exactly."
        );
    }

    #[test]
    fn confirm_label_names_the_mapping_mode_and_keeps_the_stale_wording() {
        assert_eq!(
            confirm_label(AlignMode::Anchored, false),
            "Sync Based On Chapters"
        );
        assert_eq!(
            confirm_label(AlignMode::Anchored, true),
            "Save order — Sync Based On Chapters"
        );
        assert_eq!(
            confirm_label(AlignMode::Mismatch, false),
            "Sync Based Off Percentage"
        );
        assert_eq!(
            confirm_label(AlignMode::NoMarks, true),
            "Save order — Sync Based Off Percentage"
        );
        assert_eq!(
            confirm_label(AlignMode::Stale, false),
            "Looks right — turn on sync"
        );
        assert_eq!(
            confirm_label(AlignMode::Stale, true),
            "Save order & turn on sync"
        );
    }

    #[test]
    fn foot_note_is_omitted_for_the_percentage_modes_only() {
        assert!(foot_note(AlignMode::Anchored).is_some());
        assert!(foot_note(AlignMode::Stale).is_some());
        assert_eq!(foot_note(AlignMode::Mismatch), None);
        assert_eq!(foot_note(AlignMode::NoMarks), None);
    }

    #[test]
    fn fmt_hm_floors_and_renders_hours_and_bare_minutes() {
        assert_eq!(fmt_hm(23_460.0), "6h 31m");
        assert_eq!(fmt_hm(2_460.0), "41m");
        assert_eq!(fmt_hm(0.0), "0m");
        assert_eq!(fmt_hm(-5.0), "0m");
        // 59:59 must not over-report as an hour.
        assert_eq!(fmt_hm(3_599.0), "59m");
    }

    #[test]
    fn interpolate_pairs_bends_through_anchors_and_defaults_linear() {
        let pairs = [(0.5, 0.7)];
        assert!((interpolate_pairs(&pairs, 0.25) - 0.35).abs() < 1e-9);
        assert!((interpolate_pairs(&pairs, 0.5) - 0.7).abs() < 1e-9);
        assert!((interpolate_pairs(&pairs, 0.75) - 0.85).abs() < 1e-9);
        assert!((interpolate_pairs(&[], 0.42) - 0.42).abs() < 1e-9);
    }

    #[test]
    fn basename_keeps_the_distinguishing_tail() {
        assert_eq!(
            basename("Brandon Sanderson/Wind and Truth/WaT (1 of 5).m4b"),
            "WaT (1 of 5).m4b"
        );
        assert_eq!(basename("plain.m4b"), "plain.m4b");
    }

    #[test]
    fn tick_step_thins_dense_lanes_and_keeps_sparse_ones() {
        assert_eq!(tick_step(31), 1);
        assert_eq!(tick_step(196), 5);
        assert_eq!(tick_step(0), 1);
    }
}
