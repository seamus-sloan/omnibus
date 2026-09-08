//! The marquee hero's sync readout: one mono line under the position ruler that
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

/// The follow switch on a linked book: whether opening one format jumps to
/// the spot the other reached, or leaves each format where it was. Off
/// keeps the confirmed alignment — it is not a disguised unlink, which is
/// why it sits on the sync line rather than in the modal's destructive row.
///
/// Configuration-shaped (rule 08 test 1): a direct call that never queues,
/// with the failure rendered rather than swallowed. `view_seq` counts the
/// alignment fetches that actually landed, which is what lets the switch
/// tell "the server has not answered yet" from "the server now says
/// something else".
#[component]
fn BdFollowToggle(
    uuid: String,
    follow: bool,
    view_seq: u32,
    on_changed: EventHandler<()>,
) -> Element {
    let mut busy = use_signal(|| false);
    let mut error = use_signal(|| None::<String>);
    // The value this switch last wrote and the server acknowledged, tagged
    // with the view it was written against. It stands in for the `follow`
    // prop until a *fresher* view lands, because that prop only moves when
    // the parent's refetch succeeds and the refetch keeps the previous view
    // on failure — without this a dropped refetch leaves the switch reading
    // the pre-flip value with no error, and every later click recomputes the
    // same target from it.
    let mut written = use_signal(|| None::<(u32, bool)>);
    // Any newer view supersedes it, matching value or not: the server can
    // move for reasons that are not this switch — confirming the alignment
    // re-arms follow — and waiting for agreement would pin the label to a
    // value the reader has since overridden by another route.
    if written().is_some_and(|(seq, _)| seq != view_seq) {
        written.set(None);
    }
    let shown = written().map_or(follow, |(_, v)| v);
    let target = !shown;
    let online = crate::pages::listen::sync_prompt::browser_online();

    let toggle_uuid = uuid.clone();
    let handle_toggle = move |_| {
        // Re-checked here, not just in `disabled`: nothing re-renders on a
        // connectivity change, and this control first paints only after a
        // successful fetch — so the attribute alone is always a stale "yes".
        // Mirrors `SyncHereButton`, but says so rather than dead-clicking.
        if !crate::pages::listen::sync_prompt::browser_online() {
            error.set(Some("you're offline — reconnect to change this".into()));
            return;
        }
        let uuid = toggle_uuid.clone();
        busy.set(true);
        error.set(None);
        spawn(async move {
            match data::set_follow_mode("", &uuid, target).await {
                Ok(()) => {
                    // Before releasing `busy`, so the next click computes its
                    // target from what the server just took rather than from
                    // a prop the refetch has not caught up to yet.
                    written.set(Some((view_seq, target)));
                    busy.set(false);
                    on_changed.call(());
                }
                Err(e) => {
                    busy.set(false);
                    error.set(Some(e.to_string()));
                }
            }
        });
    };

    rsx! {
        button {
            class: "bdmq-synclink bdmq-followtoggle",
            r#type: "button",
            role: "switch",
            "aria-checked": if shown { "true" } else { "false" },
            // A stable accessible name while the visible label flips, so a
            // caller can name the control without naming its state.
            "aria-label": "Follow the other format",
            title: "Following jumps to the spot the other format reached. Off keeps the alignment you confirmed and stops the jumps \u{2014} Unlink is what discards it.",
            "data-testid": "sync-follow-toggle",
            disabled: busy() || !online,
            onclick: handle_toggle,
            if shown { "following" } else { "not following" }
        }
        if let Some(msg) = error() {
            span {
                class: "bdmq-syncerr",
                role: "status",
                "data-testid": "sync-follow-error",
                "{msg}"
            }
        }
    }
}

/// The design's one-line readout for a loaded alignment view, with the
/// modal-opening affordance for its state. `on_changed` re-fetches after a
/// follow flip, so the switch renders the server's answer.
fn sync_line(
    uuid: &str,
    view: &AlignmentView,
    view_seq: u32,
    mut open_modal: Signal<bool>,
    on_changed: EventHandler<()>,
) -> Element {
    match &view.link {
        None => rsx! {
            div { class: "mono bdmq-syncline",
                "\u{21c4} positions aren't synced \u{2014} each format keeps its own spot"
                button {
                    class: "bdmq-synclink",
                    r#type: "button",
                    "data-testid": "sync-link-open",
                    onclick: move |_| open_modal.set(true),
                    "Link Formats"
                }
            }
        },
        Some(l) if l.stale => rsx! {
            div { class: "mono bdmq-syncline bdmq-syncline-warn", role: "status",
                "\u{21c4} the audiobook files changed since you linked \u{2014} sync is paused"
                button {
                    class: "bdmq-synclink",
                    r#type: "button",
                    "data-testid": "sync-link-review",
                    onclick: move |_| open_modal.set(true),
                    "Review Alignment"
                }
            }
        },
        Some(l) => {
            let at = linked_audio_at(view)
                .map(|at| format!(" \u{2014} audio at {at}"))
                .unwrap_or_default();
            let when = recency(l.confirmed_at);
            rsx! {
                div { class: "mono bdmq-syncline",
                    "\u{21c4} one spot, both formats{at} \u{b7} confirmed {when}"
                    BdFollowToggle {
                        uuid: uuid.to_string(),
                        follow: l.follow,
                        view_seq,
                        on_changed,
                    }
                    button {
                        class: "bdmq-synclink",
                        r#type: "button",
                        "data-testid": "sync-link-manage",
                        onclick: move |_| open_modal.set(true),
                        "Manage Ebook & Audiobook Sync"
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
/// `view_seq` counts the fetches that actually landed — bumped only beside
/// a `view.set`, never on a bare attempt, so the follow switch can tell a
/// refetch that failed from one that came back with a different answer.
fn use_fetch_alignment(
    uuid: String,
    refresh: Signal<u32>,
    epoch: Signal<u32>,
    mut view: Signal<Option<AlignmentView>>,
    mut fetch_failed: Signal<bool>,
    mut view_seq: Signal<u32>,
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
                    view_seq.set(view_seq() + 1);
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
    marquee: bool,
) -> Element {
    let _ = marquee;
    // None = state unknown (SSR / first paint); Some is the fetched view.
    let view = use_signal(|| None::<AlignmentView>);
    let fetch_failed = use_signal(|| false);
    let modal_open = use_signal(|| false);
    let mut epoch = use_signal(|| 0u32);
    let view_seq = use_signal(|| 0u32);
    use_fetch_alignment(uuid.clone(), refresh, epoch, view, fetch_failed, view_seq);

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
                    div { class: "mono bdmq-syncline", role: "status",
                        "couldn't load the sync status"
                        button {
                            class: "bdmq-synclink",
                            r#type: "button",
                            "data-testid": "sync-link-retry",
                            onclick: move |_| epoch.set(epoch() + 1),
                            "Retry"
                        }
                    }
                },
                None => rsx! {},
                Some(v) => sync_line(
                    &uuid,
                    &v,
                    view_seq(),
                    modal_open,
                    EventHandler::new(move |_| epoch.set(epoch() + 1)),
                ),
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
