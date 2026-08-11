//! Admin server-health page — the first admin route in the app. Five
//! read-only sections over one [`AdminHealthReport`] fetch, polled every
//! [`POLL_INTERVAL_MS`] from a post-mount `use_future` loop (never the
//! render body, for hydration parity — see #955). Web/SSR-only; the real
//! authorization boundary is the `AdminUser` extractor on
//! `rpc_get_admin_health`.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use omnibus_shared::admin_health::{
    AdminHealthReport, FtsHealth, IndexStatus, StorageStats, WorkerQueueEntry,
};
use omnibus_shared::error_ring::CapturedError;

use crate::components::worker_status::kind_label;
use crate::data::{self, DataError};
use crate::platform_sleep::async_sleep_ms;

/// Polling cadence in milliseconds — AC1 asks for "approximately a
/// 5-second cadence". Same self-hosted-single-admin reasoning as
/// `worker_status::POLL_INTERVAL_MS`: no idle-throttling, the report is
/// small and this page has one viewer at a time.
const POLL_INTERVAL_MS: u32 = 5_000;

/// The `/admin/health` page.
#[component]
pub fn AdminHealthPage() -> Element {
    let is_admin = crate::use_is_admin();
    let result = use_signal(|| None::<AdminHealthReport>);
    let error = use_signal(|| false);

    spawn_admin_health_fetch(result, error);

    rsx! {
        div { class: "ah-page", "data-testid": "admin-health-page",
            header { class: "ah-head",
                h1 { "Server health" }
                p { class: "subtitle",
                    "Index status, worker queue, search index, storage, and recent errors."
                }
            }
            if is_admin() {
                AdminHealthBody { result, error }
            } else {
                p { class: "settings-status error", "data-testid": "admin-health-forbidden",
                    "Administrator access is required to view server health."
                }
            }
        }
    }
}

/// Poll the combined report every [`POLL_INTERVAL_MS`] from a post-mount
/// `use_future` loop (#955 AC1/AC2) — fetch, apply, sleep, repeat. Dioxus
/// cancels the future when the page unmounts (AC4), same guarantee
/// `components::worker_status::WorkerStatusIndicator` relies on for its own
/// 1 Hz loop, so no explicit `use_drop` teardown is needed.
fn spawn_admin_health_fetch(
    mut result: Signal<Option<AdminHealthReport>>,
    mut error: Signal<bool>,
) {
    use_future(move || async move {
        loop {
            let outcome = data::get_admin_health().await;
            apply_poll_result(outcome, &mut result, &mut error);
            async_sleep_ms(POLL_INTERVAL_MS).await;
        }
    });
}

/// Merge one poll's outcome into the `result`/`error` signals. A success
/// always replaces the report and clears any prior error. A failure only
/// flips the page to its error state when there is no report to keep
/// showing yet (the first load) — a later transient blip leaves the last
/// known-good report on screen rather than flashing the whole page to an
/// error, mirroring `WorkerStatusIndicator`'s "keep the last known
/// snapshot" behavior.
fn apply_poll_result(
    outcome: Result<AdminHealthReport, DataError>,
    result: &mut Signal<Option<AdminHealthReport>>,
    error: &mut Signal<bool>,
) {
    match outcome {
        Ok(report) => {
            result.set(Some(report));
            error.set(false);
        }
        Err(_) if result.peek().is_none() => error.set(true),
        Err(_) => {}
    }
}

/// The result region: loading / error / populated states.
#[component]
fn AdminHealthBody(result: Signal<Option<AdminHealthReport>>, error: Signal<bool>) -> Element {
    if error() {
        return rsx! {
            p { class: "settings-status error", role: "status", "data-testid": "admin-health-error",
                "Failed to load server health."
            }
        };
    }
    let Some(report) = result() else {
        return rsx! {
            p { class: "settings-status", role: "status", "data-testid": "admin-health-loading",
                "Loading\u{2026}"
            }
        };
    };
    // `report` is an owned local (already cloned out of the signal by
    // `result()`), and each field below is used exactly once, so a plain
    // partial move covers every section without a further clone.
    rsx! {
        div { class: "ah-grid", "data-testid": "admin-health-grid",
            IndexStatusCard { status: report.index }
            WorkerQueueCard { entries: report.worker_queue }
            FtsHealthCard { fts: report.fts }
            StorageCard { storage: report.storage }
        }
        LastErrorsCard { entries: report.last_errors }
    }
}

// ── Index status ─────────────────────────────────────────────────

/// Section 1: configured library paths and row counts.
#[component]
fn IndexStatusCard(status: IndexStatus) -> Element {
    rsx! {
        section { class: "card ah-card", "data-testid": "admin-health-index-card",
            h2 { "Index status" }
            div { class: "ah-kv",
                AhKvRow { label: "Ebook library", value: path_or_unconfigured(status.ebook_library_path.as_deref()) }
                AhKvRow { label: "Audiobook library", value: path_or_unconfigured(status.audiobook_library_path.as_deref()) }
                AhKvRow { label: "Books indexed", value: status.book_count.to_string() }
                AhKvRow { label: "Authors indexed", value: status.author_count.to_string() }
                AhKvRow { label: "Series indexed", value: status.series_count.to_string() }
            }
            if status.ghosted_book_count > 0 {
                p { class: "settings-status error", "data-testid": "admin-health-ghosted-warning",
                    "{status.ghosted_book_count} book(s) have no file on disk (ghosted by a prior reindex)."
                }
            } else {
                p { class: "settings-status success", "data-testid": "admin-health-ghosted-ok",
                    "No ghosted books."
                }
            }
        }
    }
}

fn path_or_unconfigured(path: Option<&str>) -> String {
    match path {
        Some(p) if !p.trim().is_empty() => p.to_string(),
        _ => "Not configured".to_string(),
    }
}

// ── Worker queue ─────────────────────────────────────────────────

/// Section 2: live per-`TaskKind` queue depth and recent completion timings.
#[component]
fn WorkerQueueCard(entries: Vec<WorkerQueueEntry>) -> Element {
    rsx! {
        section { class: "card ah-card", "data-testid": "admin-health-worker-card",
            h2 { "Worker queue" }
            if entries.is_empty() {
                p { class: "settings-status", "data-testid": "admin-health-worker-empty",
                    "No active or recently completed background tasks."
                }
            } else {
                table { class: "users-table", "data-testid": "admin-health-worker-table",
                    thead {
                        tr {
                            th { "Task" }
                            th { "Queued / running" }
                            th { "Recent completions" }
                            th { "Last duration" }
                        }
                    }
                    tbody {
                        for entry in entries {
                            WorkerQueueRow { key: "{entry.kind:?}", entry }
                        }
                    }
                }
            }
        }
    }
}

#[component]
fn WorkerQueueRow(entry: WorkerQueueEntry) -> Element {
    let label = kind_label(entry.kind, entry.queue_depth > 0);
    let last_ms = entry.recent_completions_ms.last().copied();
    rsx! {
        tr { "data-testid": "admin-health-worker-row",
            td { "{label}" }
            td { "{entry.queue_depth}" }
            td { "{entry.recent_completions_ms.len()}" }
            td {
                match last_ms {
                    Some(ms) => rsx! { "{ms} ms" },
                    None => rsx! { "\u{2014}" },
                }
            }
        }
    }
}

// ── FTS index health ─────────────────────────────────────────────

/// Section 3: `books_fts` vs `books` row-count parity.
#[component]
fn FtsHealthCard(fts: FtsHealth) -> Element {
    rsx! {
        section { class: "card ah-card", "data-testid": "admin-health-fts-card",
            h2 { "Search index (FTS)" }
            div { class: "ah-kv",
                AhKvRow { label: "Books rows", value: fts.books_rows.to_string() }
                AhKvRow { label: "FTS rows", value: fts.fts_rows.to_string() }
            }
            if fts.in_sync {
                p { class: "settings-status success", "data-testid": "admin-health-fts-in-sync",
                    "In sync."
                }
            } else {
                p { class: "settings-status error", "data-testid": "admin-health-fts-out-of-sync",
                    "Out of sync — a rebuild is recommended (Settings \u{2192} Library)."
                }
            }
        }
    }
}

// ── Storage utilization ──────────────────────────────────────────

/// Section 4: disk usage across the library and every on-disk cache.
#[component]
fn StorageCard(storage: StorageStats) -> Element {
    rsx! {
        section { class: "card ah-card", "data-testid": "admin-health-storage-card",
            h2 { "Storage" }
            div { class: "ah-kv",
                AhKvRow {
                    label: "Library content",
                    value: format_bytes(u64::try_from(storage.library_bytes).unwrap_or(0)),
                }
                AhKvRow { label: "Covers", value: format_bytes(storage.covers_bytes) }
                AhKvRow {
                    label: "Thumbnail cache",
                    value: format_cache_usage(storage.thumbs_bytes, storage.thumbs_cap_bytes),
                }
                AhKvRow {
                    label: "HLS transcode cache",
                    value: format_cache_usage(storage.hls_bytes, storage.hls_cap_bytes),
                }
                AhKvRow {
                    label: "Export-EPUB cache",
                    value: format_cache_usage(storage.export_epub_bytes, storage.export_epub_cap_bytes),
                }
            }
        }
    }
}

/// `"12.3 MB / 5.0 GB cap"`.
fn format_cache_usage(used: u64, cap: u64) -> String {
    format!("{} / {} cap", format_bytes(used), format_bytes(cap))
}

/// Human-readable byte count (`"3.1 MB"`, `"0 B"`). Unlike
/// `format::file_size`, zero is a real value here (an empty cache), not an
/// absent one, so it always returns a string.
fn format_bytes(bytes: u64) -> String {
    #[allow(clippy::cast_precision_loss)]
    let (value, unit) = if bytes >= 1_000_000_000 {
        (bytes as f64 / 1_000_000_000.0, "GB")
    } else if bytes >= 1_000_000 {
        (bytes as f64 / 1_000_000.0, "MB")
    } else if bytes >= 1_000 {
        (bytes as f64 / 1_000.0, "KB")
    } else {
        return format!("{bytes} B");
    };
    format!("{value:.1} {unit}")
}

// ── Last errors ───────────────────────────────────────────────────

/// Section 5: the in-memory error ring, newest first. Same markup shape as
/// `pages::settings::health::LastErrorsSection`, but prop-driven off the
/// combined report instead of a second independent fetch.
#[component]
fn LastErrorsCard(entries: Vec<CapturedError>) -> Element {
    rsx! {
        section { class: "card ah-card", "data-testid": "admin-health-errors-card",
            h2 { "Last errors" }
            p { class: "subtitle", "Most recent server errors captured in memory, newest first." }
            if entries.is_empty() {
                p { class: "settings-status", "data-testid": "admin-health-errors-empty",
                    "No errors captured since the server started."
                }
            } else {
                div {
                    class: "logs-stream mono",
                    "data-testid": "admin-health-errors-table",
                    role: "region",
                    "aria-label": "Recent server errors",
                    for (i, entry) in entries.into_iter().enumerate() {
                        AhErrorLine { key: "{entry.timestamp}-{i}", entry }
                    }
                }
            }
        }
    }
}

#[component]
fn AhErrorLine(entry: CapturedError) -> Element {
    rsx! {
        div { class: "logs-line", "data-testid": "admin-health-errors-row",
            span { class: "logs-time", "{fmt_timestamp(entry.timestamp)}" }
            " "
            span { class: "logs-level logs-level-error", "ERROR" }
            " "
            span { class: "logs-module-cell", "{entry.target}" }
            " "
            span { class: "logs-message", "{entry.message}" }
        }
    }
}

/// One label/value row shared by the Index status, FTS, and Storage cards.
#[component]
fn AhKvRow(label: &'static str, value: String) -> Element {
    rsx! {
        div { class: "ah-kv-row",
            span { class: "ah-kv-label", "{label}" }
            span { class: "ah-kv-value", "{value}" }
        }
    }
}

/// Format a unix-seconds timestamp as `YYYY-MM-DD HH:MM:SS UTC`.
/// Dependency-free and deterministic (no wall clock, no locale), so SSR and
/// the first WASM paint render identically (rule 07). Duplicated from
/// `pages::settings::health::fmt_timestamp` — that helper is scoped
/// `pub(super)` to its own section, same rationale as
/// `pages::book_detail::dates::fmt_long_date`'s other duplicate.
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
/// (Howard Hinnant's `civil_from_days`).
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

#[cfg(all(test, feature = "server"))]
mod tests;
