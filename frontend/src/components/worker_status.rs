//! Worker progress indicator — polls `/api/rpc/worker_status` at 1 Hz and
//! renders a per-task row (active scans / thumbnails / author photos) plus
//! a transient "complete" or red error banner for terminal entries.
//! Mounted on `/settings` but agnostic of host page. Web-only; mobile gets
//! an empty-status stub so callers compile.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::{GhostFilesWarning, ProgressState, TaskKind, TaskProgress, WorkerStatus};

use crate::platform_sleep::async_sleep_ms;
use crate::{data, use_server_url};

/// Polling cadence in milliseconds. Always 1 s while the component is
/// mounted — the response is ~1 KB and this is a self-hosted single-user
/// app. Idle-throttling adds bug surface (when does it un-throttle?) for
/// no measurable benefit.
const POLL_INTERVAL_MS: u32 = 1_000;

/// 1 Hz-polled worker progress strip. Renders nothing when the worker is
/// idle so consumers can mount it inline without leaving dead space.
#[component]
pub fn WorkerStatusIndicator() -> Element {
    let server_url = use_server_url();
    let mut status = use_signal(WorkerStatus::default);
    // Client-suppressed terminal task ids — the user dismissed the
    // "Library updated" / red banner before the server evicted it.
    // Pruned on every render to ids still present in `recent_complete`
    // so the set can't grow past the server's own ~10 s retention
    // window (see snapshot loop below).
    let dismissed = use_signal(std::collections::HashSet::<u64>::new);

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
    prune_dismissed(dismissed, &snap);
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
                ActiveRow { key: "{task.task_id}", task: task.clone() }
            }
            // Recently-finished tasks. Done → inline success row that
            // fades client-side; Failed → red banner with dismiss.
            for task in visible_terminals.iter() {
                {terminal_row(task, dismissed)}
            }
        }
    }
}

/// Drop dismissed ids the server has already evicted so the set stays bounded by `recent_complete`.
fn prune_dismissed(mut dismissed: Signal<std::collections::HashSet<u64>>, snap: &WorkerStatus) {
    let current = dismissed();
    let live: std::collections::HashSet<u64> = current
        .iter()
        .copied()
        .filter(|id| snap.recent_complete.iter().any(|p| p.task_id == *id))
        .collect();
    if live.len() != current.len() {
        dismissed.set(live);
    }
}

/// Render a single terminal-state row, dispatching on `task.state` to the matching row component.
fn terminal_row(
    task: &TaskProgress,
    mut dismissed: Signal<std::collections::HashSet<u64>>,
) -> Element {
    let id = task.task_id;
    match &task.state {
        // A ghost-count warning (issue #1057) renders a distinct dismissible
        // WarnRow instead of the ordinary green DoneRow — AC1/AC3. Below the
        // warn threshold `ghost_warning` is `None` and behavior is unchanged
        // (AC2).
        ProgressState::Done {
            ghost_warning: Some(warning),
            ..
        } => rsx! {
            WarnRow {
                key: "{id}",
                kind: task.kind,
                warning: warning.clone(),
                on_dismiss: move |_| {
                    let mut set = dismissed();
                    set.insert(id);
                    dismissed.set(set);
                },
            }
        },
        ProgressState::Done {
            ghost_warning: None,
            ..
        } => rsx! {
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
        // Defensive: a Running entry shouldn't be in recent_complete,
        // but render it as active rather than panicking on the
        // unreachable arm.
        ProgressState::Running { .. } => rsx! {
            ActiveRow { key: "{id}", task: task.clone() }
        },
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

/// A scan that ghosted a large-but-sub-abort-threshold number of books
/// (issue #1057) — distinct from both the green [`DoneRow`] and the red
/// [`FailedRow`]: the scan still succeeded (files really are gone from the
/// index), but the count is large enough that a dropped mount or partial
/// sync is the more likely explanation.
#[component]
fn WarnRow(
    kind: TaskKind,
    warning: GhostFilesWarning,
    on_dismiss: EventHandler<MouseEvent>,
) -> Element {
    let label = kind_label(kind, /* running */ false);
    let message = ghost_warning_message(&warning);
    rsx! {
        div {
            class: "worker-status-row worker-status-warn",
            "data-testid": "worker-status-warn-row",
            span { class: "worker-status-icon", aria_hidden: "true", "!" }
            div { class: "worker-status-body",
                div { class: "worker-status-label", "{label} finished with a warning" }
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
/// variants add one match arm here and inherit the same active/done/failed
/// rendering for free.
fn kind_label(kind: TaskKind, running: bool) -> &'static str {
    match (kind, running) {
        (TaskKind::Scan, true) => "Scanning library",
        (TaskKind::Scan, false) => "Library scan",
        (TaskKind::GenerateThumbs, true) => "Generating thumbnail",
        (TaskKind::GenerateThumbs, false) => "Thumbnail generation",
        (TaskKind::ResolveAuthorPhoto, true) => "Resolving author photo",
        (TaskKind::ResolveAuthorPhoto, false) => "Author photo lookup",
        (TaskKind::RefetchAuthorPhotos, true) => "Refetching author photos",
        (TaskKind::RefetchAuthorPhotos, false) => "Author photo refetch",
        (TaskKind::BackfillChapters, true) => "Extracting audiobook chapters",
        (TaskKind::BackfillChapters, false) => "Audiobook chapter extraction",
        (TaskKind::ResolveSuggestions, true) => "Finding suggestions",
        (TaskKind::ResolveSuggestions, false) => "Suggestion lookup",
        // `#[non_exhaustive]` on the shared enum means new variants
        // compile against existing client builds — render an opaque
        // fallback rather than panicking.
        (_, true) => "Background task running",
        (_, false) => "Background task",
    }
}

/// Render a [`GhostFilesWarning`] into the [`WarnRow`] message text —
/// factored out of the component body so the exact wording is unit
/// testable per AC1 ("names the count and total").
fn ghost_warning_message(warning: &GhostFilesWarning) -> String {
    let GhostFilesWarning { removed, total } = warning;
    format!(
        "{removed} of {total} books lost their files in this scan — \
         check that your library path is mounted."
    )
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
            TaskKind::RefetchAuthorPhotos,
            TaskKind::BackfillChapters,
            TaskKind::ResolveSuggestions,
        ] {
            assert!(!kind_label(kind, true).is_empty());
            assert!(!kind_label(kind, false).is_empty());
        }
    }

    #[test]
    fn ghost_warning_message_names_the_removed_count_and_total() {
        let warning = GhostFilesWarning {
            removed: 15,
            total: 100,
        };
        let message = ghost_warning_message(&warning);
        assert!(
            message.contains("15"),
            "message must name the count: {message}"
        );
        assert!(
            message.contains("100"),
            "message must name the total: {message}"
        );
    }
}
