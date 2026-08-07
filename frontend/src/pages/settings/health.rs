//! Server Health settings section — the "Last errors" panel over the
//! in-memory error ring buffer. Web/SSR-only, mirroring `LogsPage`'s split:
//! the real authorization boundary is the `AdminUser` extractor on
//! `rpc_get_last_errors`, and the in-page `use_is_admin` gate just keeps the
//! chrome off a non-admin screen.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::error_ring::CapturedError;

use crate::{data, Route};

/// The Server Health "Last errors" section of `/settings`.
#[component]
pub fn LastErrorsSection() -> Element {
    let is_admin = crate::use_is_admin();

    let result = use_signal(|| None::<Vec<CapturedError>>);
    let error = use_signal(|| false);

    spawn_last_errors_fetch(result, error);

    rsx! {
        section { class: "card", "data-testid": "last-errors-card",
            div { class: "logs-head",
                div {
                    h2 { "Last errors" }
                    p { class: "subtitle",
                        "Most recent server errors captured in memory, newest first."
                    }
                }
            }
            if is_admin() {
                LastErrorsResults { result, error }
            } else {
                p { class: "settings-status error", "data-testid": "last-errors-forbidden",
                    "Administrator access is required to view server errors."
                }
            }
        }
    }
}

/// Fetch the ring-buffer snapshot on mount, mapping success/failure onto the
/// result/error signals.
fn spawn_last_errors_fetch(
    mut result: Signal<Option<Vec<CapturedError>>>,
    mut error: Signal<bool>,
) {
    use_effect(move || {
        error.set(false);
        spawn(async move {
            match data::get_last_errors().await {
                Ok(entries) => result.set(Some(entries)),
                Err(_) => error.set(true),
            }
        });
    });
}

/// The result region: loading / error / empty / populated states.
#[component]
fn LastErrorsResults(result: Signal<Option<Vec<CapturedError>>>, error: Signal<bool>) -> Element {
    if error() {
        return rsx! {
            p { class: "settings-status error", role: "status", "data-testid": "last-errors-error",
                "Failed to load recent errors."
            }
        };
    }
    let Some(entries) = result() else {
        return rsx! {
            p { class: "settings-status", role: "status", "data-testid": "last-errors-loading",
                "Loading\u{2026}"
            }
        };
    };
    if entries.is_empty() {
        return rsx! {
            p { class: "settings-status", role: "status", "data-testid": "last-errors-empty",
                "No errors captured since the server started."
            }
        };
    }
    rsx! {
        div {
            class: "logs-stream mono",
            "data-testid": "last-errors-table",
            role: "region",
            "aria-label": "Recent server errors",
            for (i, entry) in entries.into_iter().enumerate() {
                LastErrorLine { key: "{entry.timestamp}-{i}", entry }
            }
        }
    }
}

/// One captured error as a single preformatted line: timestamp, level badge,
/// module, message, and (when present) a link to the offending book or the
/// offending file's path (AC2).
#[component]
fn LastErrorLine(entry: CapturedError) -> Element {
    rsx! {
        div { class: "logs-line", "data-testid": "last-errors-row",
            span { class: "logs-time", "{fmt_timestamp(entry.timestamp)}" }
            " "
            span { class: "logs-level logs-level-error", "ERROR" }
            " "
            span { class: "logs-module-cell", "{entry.target}" }
            " "
            span { class: "logs-message", "{entry.message}" }
            if let Some(uuid) = entry.book_uuid {
                " "
                Link {
                    to: Route::BookDetail { uuid: uuid.clone() },
                    class: "logs-ref-link",
                    "data-testid": "last-errors-book-link",
                    "book \u{2192}"
                }
            }
            if let Some(file) = entry.file {
                " "
                span { class: "logs-fields", "data-testid": "last-errors-file", "{file}" }
            }
        }
    }
}

/// Format a unix-seconds timestamp as `YYYY-MM-DD HH:MM:SS UTC`.
/// Dependency-free and deterministic (no wall clock, no locale), so SSR and
/// the first WASM paint render identically (rule 07).
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
/// (Howard Hinnant's `civil_from_days`) — same algorithm as
/// `pages::book_detail::dates::fmt_long_date`, duplicated locally since
/// that helper is scoped `pub(super)` to its own section.
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
    use super::fmt_timestamp;

    #[test]
    fn fmt_timestamp_formats_a_known_epoch_second() {
        assert_eq!(fmt_timestamp(1_700_000_000), "2023-11-14 22:13:20 UTC");
    }

    #[test]
    fn fmt_timestamp_formats_the_epoch_itself() {
        assert_eq!(fmt_timestamp(0), "1970-01-01 00:00:00 UTC");
    }
}
