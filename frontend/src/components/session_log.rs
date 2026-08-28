//! Reading-session log — the per-sitting list behind the stats aggregates,
//! mounted user-wide on `/stats` and book-scoped on the book-detail Stats
//! stop. One row is one *sitting*, not one heartbeat flush: the server
//! stitches adjacent checkpoint rows before it pages them, so a two-hour read
//! is one entry rather than a hundred and twenty.

use dioxus::prelude::*;
use omnibus_shared::SessionLogEntry;

use crate::date_fmt::civil_from_days;
use crate::{data, use_server_url};

/// "Xh Ym" / "Xm" length label for a sitting. Shared with the book-detail
/// stats stop, which renders the same shape for its aggregate figures.
pub fn duration_label(secs: i64) -> String {
    let secs = secs.max(0);
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    match (hours, minutes) {
        (0, m) => format!("{m}m"),
        (h, 0) => format!("{h}h"),
        (h, m) => format!("{h}h {m}m"),
    }
}

/// `"Nov 14, 2023 · 22:13"` from unix seconds.
///
/// UTC, like every other date on the stats surfaces (the heatmap and the
/// per-book spark bucket by UTC day), and derived with no wall-clock read so
/// SSR and the first WASM paint agree (rule 07).
fn fmt_started(unix_secs: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let days = unix_secs.div_euclid(86_400);
    let secs_of_day = unix_secs.rem_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    let month = MONTHS[(m as usize).saturating_sub(1).min(11)];
    let (h, mi) = (secs_of_day / 3600, (secs_of_day % 3600) / 60);
    format!("{month} {d}, {y} \u{b7} {h:02}:{mi:02}")
}

/// The session log list.
///
/// `book` scopes it to one book — and when it is `Some`, the rows drop the
/// title: every one of them would carry the same name as the page they sit
/// on. `compact` drops the card chrome for a surface that supplies its own
/// (the book-detail marquee stop).
///
/// Paging is keyset, so "Show more" appends the page after the last row
/// already shown rather than an offset that a session landing mid-scroll
/// would shift.
#[component]
pub fn SessionLogList(book: Option<String>, compact: bool) -> Element {
    let server_url = use_server_url();
    let mut entries: Signal<Vec<SessionLogEntry>> = use_signal(Vec::new);
    let mut next_before: Signal<Option<String>> = use_signal(|| None);
    // Seeded loading on every target so SSR and the first WASM paint render
    // the same placeholder; the fetch runs post-mount (rule 07).
    let mut loading = use_signal(|| true);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    {
        let url = server_url.clone();
        let generation = crate::use_cache_generation();
        use_effect(use_reactive!(|book| {
            // Re-run on cache-revalidation bumps, so a just-finished sitting
            // appears without a reload.
            let _ = generation();
            let url = url.clone();
            let book = book.clone();
            entries.set(Vec::new());
            next_before.set(None);
            error.set(None);
            loading.set(true);
            spawn(async move {
                match data::fetch_session_log(&url, book.as_deref(), None).await {
                    Ok(page) => {
                        next_before.set(page.next_before);
                        entries.set(page.entries);
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
                loading.set(false);
            });
        }));
    }

    let on_more = {
        let url = server_url.clone();
        let book = book.clone();
        move |_| {
            let Some(cursor) = next_before() else {
                return;
            };
            if loading() {
                return;
            }
            let url = url.clone();
            let book = book.clone();
            // The page this request continues. A cache-generation bump can
            // reset the list while the fetch is in flight; without this the
            // late response would append page two onto an emptied list and
            // hand back a cursor pointing into rows nobody is showing.
            let issued_for = cursor.clone();
            loading.set(true);
            error.set(None);
            spawn(async move {
                match data::fetch_session_log(&url, book.as_deref(), Some(&cursor)).await {
                    Ok(page) => {
                        if next_before.peek().as_deref() == Some(issued_for.as_str()) {
                            next_before.set(page.next_before);
                            entries.write().extend(page.entries);
                        }
                    }
                    Err(e) => error.set(Some(e.to_string())),
                }
                loading.set(false);
            });
        }
    };

    let show_title = book.is_none();
    let first_load = loading() && entries.read().is_empty();
    let body = rsx! {
        if first_load {
            div { class: "st-log-placeholder", aria_hidden: "true" }
        } else if entries.read().is_empty() && error.read().is_none() {
            p { class: "st-log-empty", "data-testid": "session-log-empty",
                if show_title {
                    "No sittings recorded yet. Open a book and the log starts itself."
                } else {
                    "No sittings recorded yet."
                }
            }
        } else {
            ul { class: "st-log-list", role: "list", "data-testid": "session-log-list",
                for e in entries.read().iter() {
                    li {
                        key: "{e.book_uuid}-{e.started_at}",
                        class: "st-log-row",
                        "data-testid": "session-log-row",
                        div { class: "mono st-log-when", {fmt_started(e.started_at)} }
                        div { class: "st-log-mid",
                            if show_title {
                                div { class: "st-log-book", "{e.title}" }
                            }
                            div { class: "st-log-format", {e.format.label()} }
                        }
                        div { class: "mono st-log-dur", {duration_label(e.seconds)} }
                    }
                }
            }
            if next_before.read().is_some() {
                button {
                    class: "st-log-more",
                    r#type: "button",
                    "data-testid": "session-log-more",
                    disabled: loading(),
                    onclick: on_more,
                    if loading() { "Loading\u{2026}" } else { "Show more" }
                }
            }
        }
        if let Some(msg) = error() {
            p { class: "st-log-error", role: "alert", "data-testid": "session-log-error",
                "Couldn't load your session log: {msg}"
            }
        }
    };

    if compact {
        return rsx! {
            div { class: "st-log st-log-compact", "data-testid": "session-log", {body} }
        };
    }
    rsx! {
        section { class: "card st-log", "data-testid": "session-log",
            header { class: "st-log-head",
                h3 { class: "st-log-title", "Session log" }
                p { class: "st-log-sub",
                    "Every sitting, newest first \u{b7} times in UTC"
                }
            }
            {body}
        }
    }
}

#[cfg(test)]
mod tests;
