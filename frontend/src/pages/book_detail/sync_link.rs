//! "Your progress" block and cross-format sync entry panel on the web book
//! detail page: per-format progress bars (or the single newest-format bar once
//! linked), then the unlinked nudge, linked chip, or stale re-confirm warning —
//! each opening the alignment modal. SSR and the first WASM paint render the
//! same neutral shell; a post-mount effect fills in the state.

use dioxus::prelude::*;
use omnibus_shared::{AlignmentAudioFile, AlignmentView};

use crate::components::alignment_modal::{
    fmt_hm, interpolate_pairs, listening_frac, recency, AlignmentModal,
};
use crate::components::glyphs::{book_glyph, headphones_glyph};
use crate::data;

/// Whole-timeline listening fraction plus total seconds, in served file
/// order; `None` when the position is absent or unplaceable.
fn audio_frac_and_total(view: &AlignmentView) -> Option<(f64, f64)> {
    let files: Vec<&AlignmentAudioFile> = view.audio_files.iter().collect();
    let total: f64 = files.iter().map(|f| f.duration_seconds).sum();
    let frac = listening_frac(view, &files)?;
    Some((frac, total))
}

/// Ebook progress row: bar at the saved percent. Skipped when there is no
/// reading position (or its percent derivation hasn't landed yet — a bar
/// can't be drawn honestly without one).
fn ebook_row(view: &AlignmentView) -> Option<Element> {
    let reading = view.reading.as_ref()?;
    let pct = reading.percent?;
    let when = recency(reading.client_updated_at);
    Some(rsx! {
        div { class: "bd-prog-row", "data-testid": "bd-progress-ebook",
            span { class: "bd-prog-glyph", {book_glyph(13)} }
            span { class: "pbar", i { style: "width:{pct}%" } }
            span { class: "mono bd-prog-cap", "{pct}% \u{b7} {when}" }
        }
    })
}

/// Audiobook progress row: bar at the whole-timeline fraction. Skipped
/// when the position is absent or unplaceable on the timeline.
fn audio_row(view: &AlignmentView) -> Option<Element> {
    let listening = view.listening.as_ref()?;
    let (frac, total) = audio_frac_and_total(view)?;
    let pct = (frac * 100.0).round();
    let at = fmt_hm(frac * total);
    let of = fmt_hm(total);
    let when = recency(listening.client_updated_at);
    Some(rsx! {
        div { class: "bd-prog-row", "data-testid": "bd-progress-audio",
            span { class: "bd-prog-glyph", {headphones_glyph(13)} }
            span { class: "pbar", i { style: "width:{pct}%" } }
            span { class: "mono bd-prog-cap", "{at} of {of} \u{b7} {when}" }
        }
    })
}

/// Linked (non-stale) single row for the newest format, with the mapped
/// counterpart footnote. The stricter-clock side wins; a missing side
/// loses; an unplaceable newest side falls back to the other. `None` when
/// neither side can render a row.
fn newest_row(view: &AlignmentView) -> Option<Element> {
    let read_clock = view.reading.as_ref().map(|r| r.client_updated_at);
    let listen_clock = view.listening.as_ref().map(|l| l.client_updated_at);
    let audio_newest = match (read_clock, listen_clock) {
        (Some(r), Some(l)) => l > r,
        (None, Some(_)) => true,
        (Some(_), None) => false,
        (None, None) => return None,
    };
    if audio_newest {
        newest_audio_row(view).or_else(|| newest_ebook_row(view))
    } else {
        newest_ebook_row(view).or_else(|| newest_audio_row(view))
    }
}

/// Newest-format row when the audiobook holds the freshest position.
fn newest_audio_row(view: &AlignmentView) -> Option<Element> {
    let listening = view.listening.as_ref()?;
    let (frac, total) = audio_frac_and_total(view)?;
    let pct = (frac * 100.0).round();
    let at = fmt_hm(frac * total);
    let when = recency(listening.client_updated_at);
    // Map audio→text through the same anchor pairs the jump uses; the
    // pairs are stored (text, audio), so swap them for this direction.
    let swapped: Vec<(f64, f64)> = view.anchor_pairs.iter().map(|&(t, a)| (a, t)).collect();
    let mapped_pct = (interpolate_pairs(&swapped, frac) * 100.0).round() as i64;
    Some(rsx! {
        div { class: "bd-prog-row", "data-testid": "bd-progress-newest",
            span { class: "bd-prog-glyph", {headphones_glyph(13)} }
            span { class: "pbar", i { style: "width:{pct}%" } }
            span { class: "mono bd-prog-cap",
                "newest: audiobook \u{b7} {at} \u{b7} {when}"
            }
        }
        div { class: "mono bd-prog-foot",
            "one spot, both formats \u{2014} the reader opens at the matching page (\u{2248} {mapped_pct}%)"
        }
    })
}

/// Newest-format row when the ebook holds the freshest position. The
/// mapped-time parenthetical is omitted when the audio timeline is empty
/// rather than showing a wrong number.
fn newest_ebook_row(view: &AlignmentView) -> Option<Element> {
    let reading = view.reading.as_ref()?;
    let pct = reading.percent?;
    let when = recency(reading.client_updated_at);
    let total: f64 = view.audio_files.iter().map(|f| f.duration_seconds).sum();
    let mapped = (total > 0.0).then(|| {
        let audio_frac = interpolate_pairs(&view.anchor_pairs, pct as f64 / 100.0);
        fmt_hm(audio_frac * total)
    });
    let foot = match mapped {
        Some(hm) => format!(
            "one spot, both formats \u{2014} the player opens at the matching time (\u{2248} {hm})"
        ),
        None => "one spot, both formats \u{2014} the player opens at the matching time".to_string(),
    };
    Some(rsx! {
        div { class: "bd-prog-row", "data-testid": "bd-progress-newest",
            span { class: "bd-prog-glyph", {book_glyph(13)} }
            span { class: "pbar", i { style: "width:{pct}%" } }
            span { class: "mono bd-prog-cap",
                "newest: ebook \u{b7} {pct}% \u{b7} {when}"
            }
        }
        div { class: "mono bd-prog-foot", "{foot}" }
    })
}

/// The per-format progress rows: one row per format while the formats keep
/// separate spots (unlinked or stale), one newest-format row + footnote
/// once linked and fresh. Empty when no position can render a row.
fn progress_rows(view: &AlignmentView) -> Element {
    let linked_fresh = matches!(&view.link, Some(l) if !l.stale);
    if linked_fresh {
        rsx! {
            if let Some(row) = newest_row(view) {
                div { class: "bd-prog-rows", {row} }
            }
        }
    } else {
        let ebook = ebook_row(view);
        let audio = audio_row(view);
        rsx! {
            if ebook.is_some() || audio.is_some() {
                div { class: "bd-prog-rows",
                    if let Some(row) = ebook { {row} }
                    if let Some(row) = audio { {row} }
                }
            }
        }
    }
}

/// The "Your progress" block + sync entry row + its modal. Mounted only
/// for dual-format books (threaded into the hero's title column).
/// `refresh` re-fetches on the page's own reload signal; `after_merge`
/// auto-opens the modal once when a merge just produced this dual-format
/// book (the merge dialog's second entry point).
#[component]
pub(super) fn BdSyncPanel(
    uuid: String,
    refresh: Signal<u32>,
    after_merge: Signal<bool>,
) -> Element {
    // None = state unknown (SSR / first paint); Some is the fetched view.
    let mut view = use_signal(|| None::<AlignmentView>);
    let mut fetch_failed = use_signal(|| false);
    let modal_open = use_signal(|| false);
    let mut epoch = use_signal(|| 0u32);

    let fetch_uuid = uuid.clone();
    use_effect(move || {
        let _ = refresh();
        let _ = epoch();
        let uuid = fetch_uuid.clone();
        spawn(async move {
            match data::get_alignment("", &uuid).await {
                Ok(v) => {
                    fetch_failed.set(false);
                    view.set(Some(v));
                }
                // Keep any previously-fetched state; only flag the failure
                // when there is nothing better to show, so the row never
                // vanishes without a way back in (the modal is the retry).
                Err(_) => fetch_failed.set(true),
            }
        });
    });

    {
        let mut after_merge = after_merge;
        let mut modal_open = modal_open;
        use_effect(move || {
            if after_merge() {
                after_merge.set(false);
                modal_open.set(true);
            }
        });
    }

    let mut open_modal = modal_open;
    rsx! {
        section { class: "bd-sync-panel", "data-testid": "sync-link-row",
            match view() {
                None if fetch_failed() => rsx! {
                    div { class: "al-entry al-entry-unlinked", role: "status",
                        span { class: "al-entry-copy",
                            "Couldn't load the sync status."
                        }
                        button {
                            class: "btn sm",
                            "data-testid": "sync-link-retry",
                            onclick: move |_| epoch.set(epoch() + 1),
                            "Retry"
                        }
                    }
                },
                None => rsx! {},
                Some(v) => rsx! {
                    div { class: "label bd-prog-head", "Your progress" }
                    {progress_rows(&v)}
                    match v.link {
                        None => rsx! {
                            div { class: "al-entry al-entry-unlinked",
                                span { class: "al-entry-glyph",
                                    crate::components::sync_glyph::SyncGlyph { size: 14 }
                                }
                                span { class: "al-entry-copy",
                                    "Positions aren't synced — each format keeps its own spot."
                                }
                                button {
                                    class: "btn sm",
                                    "data-testid": "sync-link-open",
                                    onclick: move |_| open_modal.set(true),
                                    "Link formats…"
                                }
                            }
                        },
                        Some(l) if l.stale => rsx! {
                            div { class: "al-entry al-entry-stale", role: "status",
                                span { class: "al-entry-copy",
                                    "The audiobook files changed since you linked — sync is paused "
                                    "until you re-confirm the alignment."
                                }
                                button {
                                    class: "btn sm",
                                    "data-testid": "sync-link-review",
                                    onclick: move |_| open_modal.set(true),
                                    "Review alignment"
                                }
                            }
                        },
                        Some(l) => rsx! {
                            button {
                                class: "al-entry al-entry-linked",
                                "data-testid": "sync-link-manage",
                                onclick: move |_| open_modal.set(true),
                                span { class: "al-entry-glyph",
                                    crate::components::sync_glyph::SyncGlyph { size: 14 }
                                }
                                span { class: "al-entry-copy",
                                    if l.follow { "Positions synced — following" } else { "Positions synced" }
                                }
                                span { class: "al-entry-meta",
                                    "ebook \u{2194} audiobook \u{b7} confirmed "
                                    {recency(l.confirmed_at)}
                                }
                                span { class: "al-entry-chevron", "\u{203a}" }
                            }
                        },
                    }
                },
            }
            AlignmentModal {
                uuid: uuid.clone(),
                open: modal_open,
                on_changed: move |_| epoch.set(epoch() + 1),
            }
        }
    }
}

#[cfg(all(test, feature = "server"))]
mod tests;
