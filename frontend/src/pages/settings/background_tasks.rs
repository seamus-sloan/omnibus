//! Background Tasks settings section — the admin dashboard over the durable
//! `background_tasks` history table (migration `0070`, issue #941). Mirrors
//! `health::LastErrorsSection`'s shape: web/SSR-only, the real authorization
//! boundary is the `AdminUser` extractor on `rpc_get_background_tasks`, and
//! the in-page `use_is_admin` gate just keeps the chrome off a non-admin
//! screen.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::{BackgroundTaskRecord, BackgroundTaskStatus};

use crate::data;

/// The "Background Tasks" section of `/settings` (AC3): a table of recent
/// worker task runs with their status and timing.
#[component]
pub fn BackgroundTasksSection() -> Element {
    let is_admin = crate::use_is_admin();

    let result = use_signal(|| None::<Vec<BackgroundTaskRecord>>);
    let error = use_signal(|| false);

    spawn_background_tasks_fetch(result, error);

    rsx! {
        section { class: "card", "data-testid": "background-tasks-card",
            div { class: "logs-head",
                div {
                    h2 { "Background Tasks" }
                    p { class: "subtitle",
                        "Recent worker task runs, newest first. Survives a server restart."
                    }
                }
            }
            if is_admin() {
                BackgroundTasksResults { result, error }
            } else {
                p { class: "settings-status error", "data-testid": "background-tasks-forbidden",
                    "Administrator access is required to view background tasks."
                }
            }
        }
    }
}

/// Fetch the task history on mount, mapping success/failure onto the
/// result/error signals.
fn spawn_background_tasks_fetch(
    mut result: Signal<Option<Vec<BackgroundTaskRecord>>>,
    mut error: Signal<bool>,
) {
    use_effect(move || {
        error.set(false);
        spawn(async move {
            match data::get_background_tasks().await {
                Ok(tasks) => result.set(Some(tasks)),
                Err(_) => error.set(true),
            }
        });
    });
}

/// The result region: loading / error / empty / populated states.
#[component]
fn BackgroundTasksResults(
    result: Signal<Option<Vec<BackgroundTaskRecord>>>,
    error: Signal<bool>,
) -> Element {
    if error() {
        return rsx! {
            p { class: "settings-status error", role: "status", "data-testid": "background-tasks-error",
                "Failed to load background task history."
            }
        };
    }
    let Some(tasks) = result() else {
        return rsx! {
            p { class: "settings-status", role: "status", "data-testid": "background-tasks-loading",
                "Loading\u{2026}"
            }
        };
    };
    if tasks.is_empty() {
        return rsx! {
            p { class: "settings-status", role: "status", "data-testid": "background-tasks-empty",
                "No background tasks have run since this history began."
            }
        };
    }
    rsx! {
        div {
            class: "logs-stream mono",
            "data-testid": "background-tasks-table",
            role: "region",
            "aria-label": "Recent background tasks",
            for task in tasks {
                BackgroundTaskLine { key: "{task.id}", task }
            }
        }
    }
}

/// One task history row as a single preformatted line: status badge, kind,
/// timing, and (on failure) the error text.
#[component]
fn BackgroundTaskLine(task: BackgroundTaskRecord) -> Element {
    let (status_label, status_class) = match task.status {
        BackgroundTaskStatus::Running => ("RUNNING", "logs-level-info"),
        BackgroundTaskStatus::Success => ("SUCCESS", "logs-level-info"),
        BackgroundTaskStatus::Failed => ("FAILED", "logs-level-error"),
    };
    rsx! {
        div { class: "logs-line", "data-testid": "background-tasks-row",
            span { class: "logs-time", "{fmt_timestamp(task.started_at)}" }
            " "
            span { class: "logs-level {status_class}", "{status_label}" }
            " "
            span { class: "logs-module-cell", "{task.task_kind}" }
            " "
            span { class: "logs-fields", "data-testid": "background-tasks-duration",
                "{fmt_duration(task.started_at, task.finished_at)}"
            }
            if let Some(err) = task.error {
                " "
                span { class: "logs-message", "data-testid": "background-tasks-row-error", "{err}" }
            }
        }
    }
}

/// Render elapsed time between start and finish, or "running…" while still
/// in flight (`finished_at.is_none()`).
fn fmt_duration(started_at: i64, finished_at: Option<i64>) -> String {
    match finished_at {
        Some(finished) => format!("{}s", (finished - started_at).max(0)),
        None => "running\u{2026}".to_string(),
    }
}

/// Format a unix-seconds timestamp as `YYYY-MM-DD HH:MM:SS UTC`. Duplicated
/// from `health::fmt_timestamp` (scoped `pub(super)` to that module) —
/// dependency-free and deterministic so SSR and the first WASM paint render
/// identically (rule 07).
fn fmt_timestamp(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let h = secs_of_day / 3600;
    let mi = (secs_of_day % 3600) / 60;
    let s = secs_of_day % 60;
    format!("{y:04}-{m:02}-{d:02} {h:02}:{mi:02}:{s:02} UTC")
}

/// Convert days since the unix epoch to a `(year, month, day)` civil date
/// (Howard Hinnant's `civil_from_days`) — see `health::civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::{civil_from_days, fmt_duration, fmt_timestamp};

    #[test]
    fn fmt_timestamp_formats_a_known_epoch_second() {
        assert_eq!(fmt_timestamp(1_700_000_000), "2023-11-14 22:13:20 UTC");
    }

    #[test]
    fn fmt_duration_reports_elapsed_seconds_when_finished() {
        assert_eq!(fmt_duration(1_000, Some(1_045)), "45s");
    }

    #[test]
    fn fmt_duration_reports_running_when_not_yet_finished() {
        assert_eq!(fmt_duration(1_000, None), "running\u{2026}");
    }

    #[test]
    fn civil_from_days_round_trips_the_epoch() {
        assert_eq!(civil_from_days(0), (1970, 1, 1));
    }
}
