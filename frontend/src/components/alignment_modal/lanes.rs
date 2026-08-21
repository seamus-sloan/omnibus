//! Both lanes and the connector field between them: chapter ticks, fills,
//! labelled position chips, the anchored mapped preview, and the delta
//! sentence — the "do these line up?" judgement the modal is built around.

use dioxus::prelude::*;
use omnibus_shared::{AlignmentAudioFile, AlignmentView, CrossFormatLinkMode};

use super::copy::fmt_hm;

/// Piecewise-linear map of a text fraction through the served anchor pairs
/// (implicit `(0,0)`/`(1,1)` endpoints) — the same interpolation the jump
/// runs server-side, so the preview cannot disagree with it.
pub(crate) fn interpolate_pairs(pairs: &[(f64, f64)], frac: f64) -> f64 {
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

/// The distinguishing tail of a library-relative label: real libraries
/// nest under `Author/Book/…`, so the shared prefix carries nothing and
/// ellipsis truncation would hide the part that differs.
pub(super) fn basename(label: &str) -> &str {
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
pub(crate) fn listening_frac(view: &AlignmentView, files: &[&AlignmentAudioFile]) -> Option<f64> {
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

/// Everything the lane markup reads, measured once from the served view
/// and the user's working order.
struct LaneGeometry<'v> {
    files: Vec<&'v AlignmentAudioFile>,
    total: f64,
    reading_frac: Option<f64>,
    listen_frac: Option<f64>,
    mapped_frac: Option<f64>,
    audio_ticks: Vec<f64>,
    stagger: bool,
    delta: Option<f64>,
    no_positions: bool,
}

/// Measure the lanes: the working file order, both positions as
/// whole-timeline fractions, the mapped preview, and the audio ticks.
fn lane_geometry<'v>(
    view: &'v AlignmentView,
    order: &[i64],
    mode: CrossFormatLinkMode,
    primary: Option<i64>,
) -> LaneGeometry<'v> {
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
    let stagger = match (listen_frac, mapped_frac) {
        (Some(l), Some(m)) => (l - m).abs() < CHIP_COLLISION_FRACTION,
        _ => false,
    };
    let delta = match (listen_frac, mapped_frac, total > 0.0) {
        (Some(l), Some(m), true) => Some((m - l) * total),
        _ => None,
    };
    let no_positions = reading_frac.is_none() && listen_frac.is_none();
    LaneGeometry {
        files,
        total,
        reading_frac,
        listen_frac,
        mapped_frac,
        audio_ticks,
        stagger,
        delta,
        no_positions,
    }
}

/// Both lanes plus the connector field between them: chapter ticks, fills,
/// labelled position chips, the anchored mapped preview, and the delta
/// sentence beneath. No hooks anywhere in this tree — it's a plain
/// data-in/markup-out function, not a `#[component]` — so splitting the rsx
/// across named helpers below carries none of the hook-order risk rule 07
/// warns about; each helper is just a sub-tree builder called inline via
/// `{helper(...)}`, same pattern as `pages/series_index.rs`'s
/// `render_series_body`.
pub(super) fn render_lanes(
    view: &AlignmentView,
    order: &[i64],
    mode: CrossFormatLinkMode,
    primary: Option<i64>,
) -> Element {
    let g = lane_geometry(view, order, mode, primary);
    let (files, total) = (&g.files, g.total);
    let (reading_frac, listen_frac, mapped_frac) = (g.reading_frac, g.listen_frac, g.mapped_frac);
    let stagger = g.stagger;
    let ebook_chapters = view
        .ebook
        .as_ref()
        .map(|e| e.chapters.clone())
        .unwrap_or_default();
    let text_step = tick_step(ebook_chapters.len());
    let audio_step = tick_step(g.audio_ticks.len());
    let audio_ticks = &g.audio_ticks;

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
            {render_text_lane(reading_frac, &ebook_chapters, text_step)}
            // The connector field: faint threads join matched anchors; the
            // accent dashed thread is the reading position landing in the
            // audio — the thing the user judges before confirming.
            {render_connector(&view.anchor_pairs, reading_frac, mapped_frac)}
            {render_audio_lane(files, total, listen_frac, mapped_frac, audio_ticks, audio_step)}
            {render_position_chip_row(listen_frac, mapped_frac, total, stagger)}
            p { class: "al-lane-label",
                "Audiobook · "
                if files.len() == 1 {
                    "{fmt_hm(total)}"
                } else {
                    "{files.len()} files, played end to end · {fmt_hm(total)}"
                }
            }
            {render_delta_note(g.no_positions, g.delta, mapped_frac, total)}
        }
    }
}

/// The ebook lane: fill up to the reading position, thinned chapter ticks,
/// and the reading marker.
fn render_text_lane(
    reading_frac: Option<f64>,
    ebook_chapters: &[omnibus_shared::AlignmentEbookChapter],
    text_step: usize,
) -> Element {
    rsx! {
        div { class: "al-lane al-lane-text",
            if let Some(f) = reading_frac {
                div { class: "al-fill", style: "width: {f * 100.0}%" }
            }
            for (i, ch) in ebook_chapters.iter().enumerate() {
                if i % text_step == 0 {
                    div {
                        key: "{ch.title}-{ch.percent}",
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
    }
}

/// The connector field between the two lanes: one thread per matched anchor
/// pair, plus the dashed accent thread for the reading position's mapped
/// landing spot.
fn render_connector(
    anchor_pairs: &[(f64, f64)],
    reading_frac: Option<f64>,
    mapped_frac: Option<f64>,
) -> Element {
    rsx! {
        div { class: "al-connector", "aria-hidden": "true",
            svg {
                view_box: "0 0 1000 44",
                preserve_aspect_ratio: "none",
                for (t, a) in anchor_pairs.iter() {
                    line {
                        key: "{t}-{a}",
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
    }
}

/// The audiobook lane: file segments sized by duration, plus an overlay of
/// the fill, thinned chapter ticks, and the listening / mapped markers.
fn render_audio_lane(
    files: &[&AlignmentAudioFile],
    total: f64,
    listen_frac: Option<f64>,
    mapped_frac: Option<f64>,
    audio_ticks: &[f64],
    audio_step: usize,
) -> Element {
    rsx! {
        div { class: "al-lane al-lane-audio",
            for f in files.iter() {
                div {
                    key: "{f.book_file_id}",
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
                        div { key: "{i}", class: "al-tick", style: "left: {t * 100.0}%" }
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
    }
}

/// The listening / mapped position chip row beneath the audio lane,
/// staggered onto two rows when the two chips would otherwise collide.
fn render_position_chip_row(
    listen_frac: Option<f64>,
    mapped_frac: Option<f64>,
    total: f64,
    stagger: bool,
) -> Element {
    rsx! {
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
    }
}

/// The judgement sentence beneath both lanes: no positions yet, the
/// measured delta between listening and mapped-reading, or (no listening
/// position at all) the bare mapped landing spot.
fn render_delta_note(
    no_positions: bool,
    delta: Option<f64>,
    mapped_frac: Option<f64>,
    total: f64,
) -> Element {
    rsx! {
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

#[cfg(test)]
mod tests {
    use super::{basename, interpolate_pairs, tick_step};

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
