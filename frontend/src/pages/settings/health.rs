//! Server Health settings section — the "Last errors" panel over the
//! in-memory error ring buffer. Web/SSR-only, mirroring `LogsPage`'s split:
//! the real authorization boundary is the `AdminUser` extractor on
//! `rpc_get_last_errors`, and the in-page `use_is_admin` gate just keeps the
//! chrome off a non-admin screen.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::error_ring::CapturedError;

use crate::date_fmt::fmt_timestamp;
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
