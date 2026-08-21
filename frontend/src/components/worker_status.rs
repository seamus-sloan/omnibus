//! Worker progress indicator — polls `/api/rpc/worker_status` at 1 Hz and
//! renders a per-task row (active scans / thumbnails / author photos) plus
//! a transient "complete" or red error banner for terminal entries.
//! Mounted on `/settings` but agnostic of host page. Web-only; mobile gets
//! an empty-status stub so callers compile.

#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::{
    GhostFilesWarning, ProgressState, ScanTallies, TaskKind, TaskProgress, WorkerStatus,
};

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
        // A fleet-wide EPUB override bake that left per-book failures
        // (#1739) — mutually exclusive with `ghost_warning`, so this arm
        // only ever matches a `RewriteAllEpubs` run.
        ProgressState::Done {
            bake_errors: Some(errors),
            ..
        } if !errors.is_empty() => rsx! {
            BakeErrorsRow {
                key: "{id}",
                errors: errors.clone(),
                on_dismiss: move |_| {
                    let mut set = dismissed();
                    set.insert(id);
                    dismissed.set(set);
                },
            }
        },
        ProgressState::Done { .. } => rsx! {
            DoneRow {
                key: "{id}",
                kind: task.kind,
                tallies: task.detail.as_ref().and_then(|d| d.tallies),
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
    // Data-driven conditionals, never cfg gates — SSR and hydration must match.
    let detail = task.detail.as_ref();
    let phase = detail.and_then(|d| d.phase.as_deref());
    let current_item = detail.and_then(|d| d.current_item.as_deref());
    let tallies_text = detail
        .and_then(|d| d.tallies.as_ref())
        .map(active_tallies_line);
    rsx! {
        div { class: "worker-status-row worker-status-active",
            span { class: "worker-status-spinner", aria_hidden: "true" }
            div { class: "worker-status-body",
                div { class: "worker-status-label",
                    "{label}"
                    if let Some(suffix) = count_suffix {
                        "{suffix}"
                    }
                }
                if let Some(phase) = phase {
                    div {
                        class: "worker-status-phase",
                        "data-testid": "worker-status-phase",
                        "{phase}"
                    }
                }
                if let Some(item) = current_item {
                    div {
                        class: "worker-status-item",
                        "data-testid": "worker-status-item",
                        "{item}"
                    }
                }
                if let Some(tallies) = tallies_text {
                    div {
                        class: "worker-status-tallies",
                        "data-testid": "worker-status-tallies",
                        "{tallies}"
                    }
                }
            }
        }
    }
}

#[component]
fn DoneRow(
    kind: TaskKind,
    tallies: Option<ScanTallies>,
    on_dismiss: EventHandler<MouseEvent>,
) -> Element {
    let label = kind_label(kind, /* running */ false);
    let summary = tallies.as_ref().map(tallies_changes);
    rsx! {
        div { class: "worker-status-row worker-status-done",
            span { class: "worker-status-icon", aria_hidden: "true", "✓" }
            div { class: "worker-status-body",
                div { class: "worker-status-label", "{label}" }
                if let Some(summary) = summary {
                    div {
                        class: "worker-status-message",
                        "data-testid": "worker-status-summary",
                        "{summary}"
                    }
                }
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

/// A fleet-wide EPUB override bake (#959, #1718) that left one or more
/// books unbaked — distinct from both [`WarnRow`] and the red
/// [`FailedRow`]: the run itself succeeded (every book was attempted, and
/// the ones that could be baked were), but the admin who kicked it off from
/// "Bake Overrides Into EPUBs" needs to know which books still need
/// attention (#1739). Doesn't route through `kind_label` — `RewriteAllEpubs`
/// reuses `TaskKind::Scan` for its wire kind (no dedicated progress
/// widget), so this row names the job directly instead of rendering the
/// misleading "Library scan" label. `errors` is just the failed
/// `book_uuid`s — the wire type deliberately drops the per-book error text
/// (which can carry a server filesystem path); see the doc comment on
/// `omnibus_shared::ProgressState::Done::bake_errors`.
#[component]
fn BakeErrorsRow(errors: Vec<String>, on_dismiss: EventHandler<MouseEvent>) -> Element {
    let message = bake_errors_message(&errors);
    rsx! {
        div {
            class: "worker-status-row worker-status-warn",
            "data-testid": "worker-status-bake-errors-row",
            span { class: "worker-status-icon", aria_hidden: "true", "!" }
            div { class: "worker-status-body",
                div { class: "worker-status-label", "EPUB override bake finished with errors" }
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
/// rendering for free. `pub(crate)` so `pages::admin_health`'s worker-queue
/// section can reuse the same labels instead of a second mapping (#952).
pub(crate) fn kind_label(kind: TaskKind, running: bool) -> &'static str {
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

/// Render the change buckets of a [`ScanTallies`] for the UI — zero
/// buckets omitted, and an all-quiet scan reads "no changes" instead of
/// an empty string. Factored out so the exact wording is unit testable.
fn tallies_changes(tallies: &ScanTallies) -> String {
    let mut parts: Vec<String> = Vec::new();
    for (count, noun) in [
        (tallies.new, "new"),
        (tallies.changed, "changed"),
        (tallies.removed, "removed"),
        (tallies.moved, "moved"),
    ] {
        if count > 0 {
            parts.push(format!("{count} {noun}"));
        }
    }
    if parts.is_empty() {
        "no changes".to_string()
    } else {
        parts.join(" · ")
    }
}

/// The active panel's tally line: the walk's file count plus
/// [`tallies_changes`].
fn active_tallies_line(tallies: &ScanTallies) -> String {
    format!("Found {} — {}", tallies.found, tallies_changes(tallies))
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

/// Maximum number of failed `book_uuid`s named inline in
/// [`bake_errors_message`] before the rest collapse into an "and N more"
/// tail — keeps the row readable for a large fleet-wide bake.
const BAKE_ERRORS_INLINE_CAP: usize = 5;

/// Render a fleet-wide EPUB bake's failed `book_uuid`s into the
/// [`BakeErrorsRow`] message text (#1739) — factored out of the component
/// body so the wording (count + the failed book uuids) is unit testable,
/// mirroring [`ghost_warning_message`].
fn bake_errors_message(errors: &[String]) -> String {
    let n = errors.len();
    let noun = if n == 1 { "book" } else { "books" };
    let uuids: Vec<&str> = errors
        .iter()
        .take(BAKE_ERRORS_INLINE_CAP)
        .map(String::as_str)
        .collect();
    let listed = uuids.join(", ");
    if n > BAKE_ERRORS_INLINE_CAP {
        format!(
            "{n} {noun} failed to bake: {listed}, and {} more.",
            n - BAKE_ERRORS_INLINE_CAP
        )
    } else {
        format!("{n} {noun} failed to bake: {listed}.")
    }
}

#[cfg(test)]
mod tests;
