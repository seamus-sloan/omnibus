//! The W4 hero's sync readout: one mono line under the position ruler that
//! says where the two formats stand — linked ("one spot, both formats"),
//! stale, or unlinked — with the affordance that opens the alignment modal.
//! SSR and the first WASM paint render the same neutral shell; a post-mount
//! effect fills in the state.

use dioxus::prelude::*;
use omnibus_shared::{AlignmentAudioFile, AlignmentView};

use crate::components::alignment_modal::{
    fmt_hm, interpolate_pairs, listening_frac, recency, AlignmentModal,
};
use crate::data;

/// Whole-timeline listening fraction plus total seconds, in served file
/// order; `None` when the position is absent or unplaceable.
fn audio_frac_and_total(view: &AlignmentView) -> Option<(f64, f64)> {
    let files: Vec<&AlignmentAudioFile> = view.audio_files.iter().collect();
    let total: f64 = files.iter().map(|f| f.duration_seconds).sum();
    let frac = listening_frac(view, &files)?;
    Some((frac, total))
}

/// The audio-timeline position named in the linked line: the saved listening
/// position when there is one, else the reading position mapped onto the
/// audio timeline through the anchor pairs (the same mapping the player-open
/// jump uses). `None` when the book has no audio timeline to speak of.
fn linked_audio_at(view: &AlignmentView) -> Option<String> {
    if let Some((frac, total)) = audio_frac_and_total(view) {
        return Some(fmt_hm(frac * total));
    }
    let total: f64 = view.audio_files.iter().map(|f| f.duration_seconds).sum();
    if total <= 0.0 {
        return None;
    }
    let pct = view.reading.as_ref()?.percent?;
    let audio_frac = interpolate_pairs(&view.anchor_pairs, pct as f64 / 100.0);
    Some(fmt_hm(audio_frac * total))
}

/// The design's one-line readout for a loaded alignment view, with the
/// modal-opening affordance for its state.
fn sync_line(view: &AlignmentView, mut open_modal: Signal<bool>) -> Element {
    match &view.link {
        None => rsx! {
            div { class: "mono bdw4-syncline",
                "\u{21c4} positions aren't synced \u{2014} each format keeps its own spot"
                button {
                    class: "bdw4-synclink",
                    r#type: "button",
                    "data-testid": "sync-link-open",
                    onclick: move |_| open_modal.set(true),
                    "link formats \u{2192}"
                }
            }
        },
        Some(l) if l.stale => rsx! {
            div { class: "mono bdw4-syncline bdw4-syncline-warn", role: "status",
                "\u{21c4} the audiobook files changed since you linked \u{2014} sync is paused"
                button {
                    class: "bdw4-synclink",
                    r#type: "button",
                    "data-testid": "sync-link-review",
                    onclick: move |_| open_modal.set(true),
                    "review alignment \u{2192}"
                }
            }
        },
        Some(l) => {
            let at = linked_audio_at(view)
                .map(|at| format!(" \u{2014} audio at {at}"))
                .unwrap_or_default();
            let when = recency(l.confirmed_at);
            let follow = if l.follow { " \u{b7} following" } else { "" };
            rsx! {
                div { class: "mono bdw4-syncline",
                    "\u{21c4} one spot, both formats{at} \u{b7} confirmed {when}{follow}"
                    button {
                        class: "bdw4-synclink",
                        r#type: "button",
                        "data-testid": "sync-link-manage",
                        onclick: move |_| open_modal.set(true),
                        "manage \u{2192}"
                    }
                }
            }
        }
    }
}

/// Fetch/refetch the alignment view on mount, on `refresh`'s signal, and on
/// `epoch` bumps (retry / post-modal-change). Keeps any previously-fetched
/// state on failure — only flags it when there is nothing better to show,
/// so the row never vanishes without a way back in (the modal is the retry).
fn use_fetch_alignment(
    uuid: String,
    refresh: Signal<u32>,
    epoch: Signal<u32>,
    mut view: Signal<Option<AlignmentView>>,
    mut fetch_failed: Signal<bool>,
) {
    use_effect(move || {
        let _ = refresh();
        let _ = epoch();
        let uuid = uuid.clone();
        spawn(async move {
            match data::get_alignment("", &uuid).await {
                Ok(v) => {
                    fetch_failed.set(false);
                    view.set(Some(v));
                }
                Err(_) => fetch_failed.set(true),
            }
        });
    });
}

/// The sync readout + its modal. Mounted only for dual-format books (under
/// the hero's ruler). `refresh` re-fetches on the page's own reload signal;
/// `after_merge` auto-opens the modal once when a merge just produced this
/// dual-format book (the merge dialog's second entry point).
#[component]
pub(super) fn BdSyncPanel(
    uuid: String,
    refresh: Signal<u32>,
    after_merge: Signal<bool>,
    /// Accepted for call-site clarity; the compact line is the only web
    /// rendering now.
    #[props(default = true)]
    w4: bool,
) -> Element {
    let _ = w4;
    // None = state unknown (SSR / first paint); Some is the fetched view.
    let view = use_signal(|| None::<AlignmentView>);
    let fetch_failed = use_signal(|| false);
    let modal_open = use_signal(|| false);
    let mut epoch = use_signal(|| 0u32);
    use_fetch_alignment(uuid.clone(), refresh, epoch, view, fetch_failed);

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

    rsx! {
        section { class: "bd-sync-panel", "data-testid": "sync-link-row",
            match view() {
                None if fetch_failed() => rsx! {
                    div { class: "mono bdw4-syncline", role: "status",
                        "couldn't load the sync status"
                        button {
                            class: "bdw4-synclink",
                            r#type: "button",
                            "data-testid": "sync-link-retry",
                            onclick: move |_| epoch.set(epoch() + 1),
                            "retry \u{2192}"
                        }
                    }
                },
                None => rsx! {},
                Some(v) => sync_line(&v, modal_open),
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
