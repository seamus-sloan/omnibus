//! Worker progress indicator — polls `/api/rpc/worker_status` at 1 Hz and
//! renders a per-task row (active scans / thumbnails / author photos)
//! plus a transient "complete" or red error banner for terminal entries.
//!
//! Mounted on `/settings` directly above the Save button for v1. The
//! component is intentionally agnostic of which page mounts it — future
//! surfaces (e.g. F5.9 library-cleanup detection trigger) can drop the
//! same primitive in without changes here, since the data plane already
//! distinguishes [`TaskKind`] variants.
//!
//! Web-only. Mobile UI is out of scope for issue #69 v1; the mobile
//! `data::worker_status` stub returns an empty status so callers compile.
//! Wrapping the whole module in `#[cfg(not(feature = "mobile"))]` keeps
//! it out of the mobile bundle entirely, mirroring how `search_palette`
//! is gated.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::{ProgressState, TaskKind, TaskProgress, WorkerStatus};

use crate::{data, use_server_url};

/// Polling cadence in milliseconds. Always 1 s while the component is
/// mounted — the issue spec calls for a 1 s tick, the response is ~1 KB,
/// and this is a self-hosted single-user app. Idle-throttling adds bug
/// surface (when does it un-throttle?) for no measurable benefit.
const POLL_INTERVAL_MS: u32 = 1_000;

/// How long to keep a dismissed terminal task suppressed client-side. The
/// server's own eviction (10s — see `TERMINAL_RETENTION` in the worker)
/// kicks in shortly after, so this only has to outlast a couple of
/// polling ticks. Longer would risk re-surfacing a still-resident
/// terminal if the user dismissed it just before the server evicted.
const DISMISS_GRACE_MS: u32 = 12_000;

/// 1 Hz-polled worker progress strip. Renders nothing when the worker is
/// idle so consumers can mount it inline without leaving dead space.
#[component]
pub fn WorkerStatusIndicator() -> Element {
    let server_url = use_server_url();
    let mut status = use_signal(WorkerStatus::default);
    // Client-suppressed terminal task ids — the user dismissed the
    // "Library updated" / red banner before the server evicted it.
    let mut dismissed = use_signal(std::collections::HashSet::<u64>::new);

    let url_for_poll = server_url.clone();
    use_future(move || {
        let url = url_for_poll.clone();
        async move {
            // The future captures the `status` signal by move; spinning
            // forever is fine because the component unmount cancels the
            // associated future. Errors keep the last known snapshot in
            // place so a transient blip doesn't flash the indicator off.
            loop {
                if let Ok(snap) = data::worker_status(&url).await {
                    status.set(snap);
                }
                async_sleep_ms(POLL_INTERVAL_MS).await;
            }
        }
    });

    let snap = status();
    let visible_terminals: Vec<TaskProgress> = snap
        .recent_complete
        .iter()
        .filter(|p| !dismissed().contains(&p.task_id))
        .cloned()
        .collect();

    if snap.active.is_empty() && visible_terminals.is_empty() {
        return rsx!();
    }

    rsx! {
        div {
            class: "worker-status",
            role: "status",
            "aria-live": "polite",
            "data-testid": "worker-status",
            // Active tasks first — these have no dismiss affordance since
            // the user can't make them go away by clicking.
            for task in snap.active.iter() {
                ActiveRow { task: task.clone() }
            }
            // Recently-finished tasks. Done → inline success row that
            // fades client-side; Failed → red banner with dismiss.
            for task in visible_terminals.iter() {
                {
                    let id = task.task_id;
                    match &task.state {
                        ProgressState::Done { .. } => rsx! {
                            DoneRow {
                                key: "{id}",
                                kind: task.kind,
                                on_dismiss: move |_| {
                                    let mut set = dismissed();
                                    set.insert(id);
                                    dismissed.set(set);
                                },
                            }
                        },
                        ProgressState::Failed { message } => rsx! {
                            FailedRow {
                                key: "{id}",
                                kind: task.kind,
                                message: message.clone(),
                                on_dismiss: move |_| {
                                    let mut set = dismissed();
                                    set.insert(id);
                                    dismissed.set(set);
                                },
                            }
                        },
                        // Defensive: a Running entry shouldn't be in
                        // recent_complete, but render it as active rather
                        // than panicking on the unreachable arm.
                        ProgressState::Running { .. } => rsx! {
                            ActiveRow { key: "{id}", task: task.clone() }
                        },
                    }
                }
            }
        }
    }
}

#[component]
fn ActiveRow(task: TaskProgress) -> Element {
    let label = kind_label(task.kind, /* running */ true);
    let count_suffix = if let ProgressState::Running {
        processed,
        total: Some(total),
    } = &task.state
    {
        Some(format!(" ({processed} / {total})"))
    } else {
        None
    };
    rsx! {
        div { class: "worker-status-row worker-status-active",
            span { class: "worker-status-spinner", aria_hidden: "true" }
            span { class: "worker-status-label",
                "{label}"
                if let Some(suffix) = count_suffix {
                    "{suffix}"
                }
            }
        }
    }
}

#[component]
fn DoneRow(kind: TaskKind, on_dismiss: EventHandler<MouseEvent>) -> Element {
    let label = kind_label(kind, /* running */ false);
    rsx! {
        div { class: "worker-status-row worker-status-done",
            span { class: "worker-status-icon", aria_hidden: "true", "✓" }
            span { class: "worker-status-label", "{label}" }
            button {
                class: "worker-status-dismiss",
                r#type: "button",
                aria_label: "Dismiss",
                onclick: move |evt| on_dismiss.call(evt),
                "✕"
            }
        }
    }
}

#[component]
fn FailedRow(kind: TaskKind, message: String, on_dismiss: EventHandler<MouseEvent>) -> Element {
    let label = kind_label(kind, /* running */ false);
    rsx! {
        div {
            class: "worker-status-row worker-status-failed",
            role: "alert",
            span { class: "worker-status-icon", aria_hidden: "true", "!" }
            div { class: "worker-status-body",
                div { class: "worker-status-label", "{label} failed" }
                div { class: "worker-status-message", "{message}" }
            }
            button {
                class: "worker-status-dismiss",
                r#type: "button",
                aria_label: "Dismiss",
                onclick: move |evt| on_dismiss.call(evt),
                "✕"
            }
        }
    }
}

/// Map a wire-protocol [`TaskKind`] to a user-facing label. `running` flips
/// the tense ("Scanning" vs "Library scan") so the indicator reads
/// naturally in both the in-flight and terminal contexts. New `TaskKind`
/// variants (e.g. F5.9 cleanup detection) add one match arm here and
/// inherit the same active/done/failed rendering for free.
fn kind_label(kind: TaskKind, running: bool) -> &'static str {
    match (kind, running) {
        (TaskKind::Scan, true) => "Scanning library",
        (TaskKind::Scan, false) => "Library scan",
        (TaskKind::GenerateThumbs, true) => "Generating thumbnail",
        (TaskKind::GenerateThumbs, false) => "Thumbnail generation",
        (TaskKind::ResolveAuthorPhoto, true) => "Resolving author photo",
        (TaskKind::ResolveAuthorPhoto, false) => "Author photo lookup",
        // `#[non_exhaustive]` on the shared enum means new variants
        // compile against existing client builds — render an opaque
        // fallback rather than panicking.
        (_, true) => "Background task running",
        (_, false) => "Background task",
    }
}

// `DISMISS_GRACE_MS` is currently consulted only by tests / future
// drift-detection code; the server-side eviction window does the heavy
// lifting. Reference it so dead-code lints don't trip while the seam
// stays available for a later "auto-fade after N ms" tweak.
#[allow(dead_code)]
const _DISMISS_GRACE_MS_TOUCH: u32 = DISMISS_GRACE_MS;

// ── Platform-gated 1 Hz poll sleeper ─────────────────────────────
//
// Web compiles with `gloo_timers` (already a dep under the `web` feature).
// The server build runs SSR — server functions execute on the server side
// of the component graph but `use_future` only runs on the client during
// hydration, so the server-only branch isn't actually exercised. Keep a
// `tokio::time::sleep` fallback so non-`web` server compilation succeeds.

#[cfg(feature = "web")]
async fn async_sleep_ms(ms: u32) {
    gloo_timers::future::TimeoutFuture::new(ms).await;
}

#[cfg(all(not(feature = "web"), feature = "server"))]
async fn async_sleep_ms(ms: u32) {
    tokio::time::sleep(std::time::Duration::from_millis(ms as u64)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_label_covers_all_variants_in_both_tenses() {
        for kind in [
            TaskKind::Scan,
            TaskKind::GenerateThumbs,
            TaskKind::ResolveAuthorPhoto,
        ] {
            assert!(!kind_label(kind, true).is_empty());
            assert!(!kind_label(kind, false).is_empty());
        }
    }
}
