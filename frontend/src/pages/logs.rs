//! Admin-only log viewer (`/logs`).
//!
//! Paginated, newest-first view over the on-disk JSON logs with
//! level/module/time-range filters. Web/SSR-only — not a mobile surface — and
//! the real authorization boundary is the `AdminUser` extractor on
//! `rpc_get_logs`; the in-page `use_is_admin` gate just keeps the chrome off a
//! non-admin screen.
#![cfg(not(feature = "mobile"))]

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::logs::{LogPage, LogQuery, LogRecord};

use crate::{data, use_server_url, Route};

/// The `/logs` admin log viewer page.
#[component]
pub fn LogsPage() -> Element {
    let is_admin = crate::use_is_admin();

    // Editable filter inputs, kept separate from the applied query so typing
    // doesn't re-fetch until the operator hits Apply.
    let level = use_signal(String::new);
    let module = use_signal(String::new);
    let from = use_signal(String::new);
    let to = use_signal(String::new);

    // The query currently driving the fetch. Changing it re-runs the effect.
    let applied = use_signal(LogQuery::default);
    let result = use_signal(|| None::<LogPage>);
    let error = use_signal(|| false);
    let loading = use_signal(|| false);
    // Monotonic ticket so a slow earlier fetch can't clobber a newer one when
    // the operator clicks Apply / pagination in quick succession.
    let fetch_seq = use_signal(|| 0u32);

    spawn_logs_fetch(use_server_url(), applied, result, error, loading, fetch_seq);

    let filters = LogFilters {
        level,
        module,
        from,
        to,
    };
    let on_apply = apply_filters_handler(filters, applied);

    rsx! {
        div { class: "settings-page logs-page", "data-testid": "logs-page",
            section { class: "card",
                div { class: "logs-head",
                    div {
                        h1 { "Server logs" }
                        p { class: "subtitle", "Browse the on-disk structured logs." }
                    }
                    Link { to: Route::Settings {}, class: "btn ghost", "Back to settings" }
                }
                if is_admin() {
                    LogFilterBar { filters, on_apply: EventHandler::new(on_apply) }
                    LogResults { applied, result, error, loading }
                } else {
                    p { class: "settings-status error", "data-testid": "logs-forbidden",
                        "Administrator access is required to view server logs."
                    }
                }
            }
        }
    }
}

/// The four editable filter-input signals, grouped so handlers take one arg.
#[derive(Copy, Clone, PartialEq)]
struct LogFilters {
    level: Signal<String>,
    module: Signal<String>,
    from: Signal<String>,
    to: Signal<String>,
}

/// Fetch the applied query whenever it changes, mapping success/failure onto
/// the result/error/loading signals. `fetch_seq` is bumped per request and
/// re-checked after the await so an out-of-order (stale) response is dropped
/// instead of overwriting a newer one. Read via `peek` so bumping it doesn't
/// re-trigger this effect — only `applied` is a dependency.
fn spawn_logs_fetch(
    server_url: String,
    applied: Signal<LogQuery>,
    mut result: Signal<Option<LogPage>>,
    mut error: Signal<bool>,
    mut loading: Signal<bool>,
    mut fetch_seq: Signal<u32>,
) {
    use_effect(move || {
        let query = applied();
        let url = server_url.clone();
        let ticket = *fetch_seq.peek() + 1;
        fetch_seq.set(ticket);
        loading.set(true);
        error.set(false);
        spawn(async move {
            let outcome = data::get_logs(&url, query).await;
            // A newer request superseded this one — drop the stale result.
            if *fetch_seq.peek() != ticket {
                return;
            }
            match outcome {
                Ok(page) => {
                    result.set(Some(page));
                }
                Err(_) => {
                    error.set(true);
                }
            }
            loading.set(false);
        });
    });
}

/// Build the Apply handler: turn the editable fields into a fresh query at
/// page 0 (a new filter always restarts from the newest page).
fn apply_filters_handler(
    filters: LogFilters,
    mut applied: Signal<LogQuery>,
) -> impl FnMut(MouseEvent) {
    move |_| {
        applied.set(LogQuery {
            level: none_if_blank(&(filters.level)()),
            module: none_if_blank(&(filters.module)()),
            from: local_to_utc_prefix(&(filters.from)()),
            to: local_to_utc_prefix(&(filters.to)()),
            page: 0,
            per_page: 0,
        });
    }
}

/// Trim a filter field, mapping an empty result to `None`.
fn none_if_blank(value: &str) -> Option<String> {
    let trimmed = value.trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Convert a `datetime-local` value (local wall-clock, `YYYY-MM-DDTHH:MM`) to a
/// UTC ISO-8601 minute prefix (`YYYY-MM-DDTHH:MM`) so it lines up with the
/// server's UTC (`…Z`) log timestamps. Without this an admin in a non-UTC zone
/// would filter a window shifted by their offset. `None` for a blank/invalid
/// value. The minute precision is deliberate — the reader treats `to` as
/// inclusive of the whole minute.
#[cfg(feature = "web")]
fn local_to_utc_prefix(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return None;
    }
    // `new Date("YYYY-MM-DDTHH:MM")` (no zone) parses as local time; the ISO
    // string is then UTC. Bail on an unparsable value rather than send NaN.
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(trimmed));
    if date.get_time().is_nan() {
        return None;
    }
    Some(
        String::from(date.to_iso_string())
            .chars()
            .take(16)
            .collect(),
    )
}

/// Non-web fallback: the Apply handler only runs client-side, so this is never
/// exercised on SSR — it just has to compile. Pass the value through untrimmed
/// of timezone semantics.
#[cfg(not(feature = "web"))]
fn local_to_utc_prefix(value: &str) -> Option<String> {
    none_if_blank(value)
}

/// Level / module / time-range inputs plus the Apply button.
#[component]
fn LogFilterBar(filters: LogFilters, on_apply: EventHandler<MouseEvent>) -> Element {
    let mut level = filters.level;
    let mut module = filters.module;
    let mut from = filters.from;
    let mut to = filters.to;
    rsx! {
        div { class: "logs-filters",
            div { class: "settings-field",
                label { r#for: "logs-level", "Level" }
                select {
                    id: "logs-level",
                    "data-testid": "logs-level",
                    value: "{level}",
                    onchange: move |e| level.set(e.value()),
                    option { value: "", "All levels" }
                    option { value: "ERROR", "Error" }
                    option { value: "WARN", "Warn" }
                    option { value: "INFO", "Info" }
                    option { value: "DEBUG", "Debug" }
                    option { value: "TRACE", "Trace" }
                }
            }
            div { class: "settings-field",
                label { r#for: "logs-module", "Module" }
                input {
                    r#type: "text",
                    id: "logs-module",
                    "data-testid": "logs-module",
                    autocomplete: "off",
                    autocapitalize: "none",
                    autocorrect: "off",
                    spellcheck: "false",
                    placeholder: "e.g. omnibus::backend",
                    value: "{module}",
                    oninput: move |e| module.set(e.value()),
                }
            }
            div { class: "settings-field",
                label { r#for: "logs-from", "From" }
                input {
                    r#type: "datetime-local",
                    id: "logs-from",
                    "data-testid": "logs-from",
                    value: "{from}",
                    oninput: move |e| from.set(e.value()),
                }
            }
            div { class: "settings-field",
                label { r#for: "logs-to", "To" }
                input {
                    r#type: "datetime-local",
                    id: "logs-to",
                    "data-testid": "logs-to",
                    value: "{to}",
                    oninput: move |e| to.set(e.value()),
                }
            }
            div { class: "settings-actions logs-apply",
                button {
                    r#type: "button",
                    class: "btn",
                    "data-testid": "logs-apply",
                    onclick: move |e| on_apply.call(e),
                    "Apply"
                }
            }
        }
    }
}

/// The result region: status line, table, and pagination controls.
#[component]
fn LogResults(
    applied: Signal<LogQuery>,
    result: Signal<Option<LogPage>>,
    error: Signal<bool>,
    loading: Signal<bool>,
) -> Element {
    if error() {
        return rsx! {
            p { class: "settings-status error", role: "status", "data-testid": "logs-error",
                "Failed to load logs."
            }
        };
    }
    let Some(page) = result() else {
        return rsx! {
            p { class: "settings-status", role: "status", "data-testid": "logs-loading", "Loading logs\u{2026}" }
        };
    };
    if page.records.is_empty() {
        return rsx! {
            p { class: "settings-status", role: "status", "data-testid": "logs-empty",
                "No log records match these filters."
            }
        };
    }
    rsx! {
        LogTable { records: page.records.clone() }
        LogPagination { applied, has_more: page.has_more, loading: loading() }
    }
}

/// Newest-first table of one page of records.
#[component]
fn LogTable(records: Vec<LogRecord>) -> Element {
    rsx! {
        div { class: "logs-table-wrap",
            table { class: "logs-table", "data-testid": "logs-table",
                thead {
                    tr {
                        th { "Time" }
                        th { "Level" }
                        th { "Module" }
                        th { "Message" }
                    }
                }
                tbody {
                    for record in records {
                        LogRow { record }
                    }
                }
            }
        }
    }
}

/// One log record row; folds the extra fields into an expandable detail.
#[component]
fn LogRow(record: LogRecord) -> Element {
    let level_class = format!("logs-level logs-level-{}", record.level.to_lowercase());
    rsx! {
        tr { class: "logs-row", "data-testid": "logs-row",
            td { class: "logs-time mono", "{record.timestamp}" }
            td {
                span { class: "{level_class}", "{record.level}" }
            }
            td { class: "logs-module-cell mono", "{record.target}" }
            td { class: "logs-message",
                "{record.message}"
                if let Some(fields) = record.fields {
                    details { class: "logs-fields",
                        summary { "details" }
                        pre { class: "mono", "{fields}" }
                    }
                }
            }
        }
    }
}

/// Prev/Next controls. Prev is disabled on the first page; Next when the
/// server reported no further matches. A page change re-triggers the fetch.
#[component]
fn LogPagination(applied: Signal<LogQuery>, has_more: bool, loading: bool) -> Element {
    let page = applied().page;
    let mut go = move |next: bool| {
        let mut query = applied();
        query.page = if next {
            query.page.saturating_add(1)
        } else {
            query.page.saturating_sub(1)
        };
        applied.set(query);
    };
    rsx! {
        div { class: "logs-pagination",
            button {
                r#type: "button",
                class: "btn ghost",
                "data-testid": "logs-prev",
                disabled: page == 0 || loading,
                onclick: move |_| go(false),
                "\u{2190} Newer"
            }
            span { class: "logs-page-indicator mono", "data-testid": "logs-page-indicator",
                "Page {page + 1}"
            }
            button {
                r#type: "button",
                class: "btn ghost",
                "data-testid": "logs-next",
                disabled: !has_more || loading,
                onclick: move |_| go(true),
                "Older \u{2192}"
            }
        }
    }
}
