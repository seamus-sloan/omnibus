//! Reading-stats page (`/stats`). Period-scoped modules up top driven by a
//! Week / Month / Year / Lifetime switcher, then an explicitly separated
//! All-time section that never re-queries on switcher change. Mirrors the
//! converged Stats design (`screens/stats-converged.jsx`).

use dioxus::prelude::*;
use omnibus_shared::{StatsRange, StatsSummary};

use crate::components::{PageError, PageLoading};
use crate::{data, use_server_url, Route};

/// The italicized period word in the page title.
fn period_word(range: StatsRange) -> &'static str {
    match range {
        StatsRange::Week => "week",
        StatsRange::Month => "month",
        StatsRange::Year => "year",
        StatsRange::AllTime => "lifetime",
    }
}

/// Compact duration for summary strips: "42 m", "3 h", "3 h 20 m".
fn format_active_time(secs: i64) -> String {
    let hours = secs / 3600;
    let minutes = (secs % 3600) / 60;
    match (hours, minutes) {
        (0, m) => format!("{m} m"),
        (h, 0) => format!("{h} h"),
        (h, m) => format!("{h} h {m} m"),
    }
}

/// Reading-stats page — period switcher, period-scoped summary, and the
/// all-time section.
#[component]
pub fn StatsPage() -> Element {
    let server_url = use_server_url();
    // Seeded to the default on every target so SSR and first WASM paint
    // match (rule 07); no localStorage seeding.
    let range = use_signal(StatsRange::default);
    let period: Signal<Option<StatsSummary>> = use_signal(|| None);
    let all_time: Signal<Option<StatsSummary>> = use_signal(|| None);
    let loading = use_signal(|| true);
    let error: Signal<Option<String>> = use_signal(|| None);
    let sheet_open = use_signal(|| false);

    use_period_fetch_effect(server_url.clone(), range, period, error);
    use_all_time_fetch_effect(server_url, all_time, loading, error);

    if loading() {
        return rsx! { PageLoading {} };
    }
    if let Some(msg) = error() {
        return rsx! { PageError { message: msg, back_to: Route::Landing {} } };
    }

    let empty = all_time.read().as_ref().is_none_or(StatsSummary::is_empty);

    rsx! {
        div { class: "st-page",
            StatsHeader { range, sheet_open }
            if empty {
                StatsEmpty {}
            } else {
                section { class: "st-period", "data-testid": "stats-period-section",
                    PeriodSummary { period }
                }
                div { class: "st-divider",
                    h3 { class: "st-divider-title", "All" span { class: "st-divider-em", "-time" } }
                    p { class: "st-divider-sub", "Not tied to the period above." }
                }
                section { class: "st-alltime", "data-testid": "stats-alltime-section",
                    AllTimeSummary { all_time }
                }
            }
            if sheet_open() {
                RangeSheet { range, sheet_open }
            }
        }
    }
}

/// Refetch the period-scoped summary whenever the switcher changes. The
/// signal read inside the effect subscribes it to `range`.
fn use_period_fetch_effect(
    server_url: String,
    range: Signal<StatsRange>,
    period: Signal<Option<StatsSummary>>,
    error: Signal<Option<String>>,
) {
    use_effect(move || {
        let r = range();
        let url = server_url.clone();
        let mut period = period;
        let mut error = error;
        spawn(async move {
            match data::fetch_stats(&url, r).await {
                Ok(summary) => period.set(Some(summary)),
                Err(e) => error.set(Some(e.to_string())),
            }
        });
    });
}

/// One-shot fetch of the all-time summary. Deliberately not keyed on the
/// switcher so the all-time section never re-queries on range change.
fn use_all_time_fetch_effect(
    server_url: String,
    all_time: Signal<Option<StatsSummary>>,
    loading: Signal<bool>,
    error: Signal<Option<String>>,
) {
    use_effect(move || {
        let url = server_url.clone();
        let mut all_time = all_time;
        let mut loading = loading;
        let mut error = error;
        spawn(async move {
            match data::fetch_stats(&url, StatsRange::AllTime).await {
                Ok(summary) => all_time.set(Some(summary)),
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    });
}

/// Editorial header: title with the italic period word, subtitle, and the
/// period switcher (desktop segmented control + mobile sheet trigger — CSS
/// picks one per form factor, never `cfg`-gated rsx).
#[component]
fn StatsHeader(range: Signal<StatsRange>, sheet_open: Signal<bool>) -> Element {
    let current = range();
    rsx! {
        header { class: "st-head",
            h1 { class: "st-title",
                "Your reading "
                span { class: "st-period-word", {period_word(current)} }
            }
            p { class: "st-sub", "Reading & listening, tracked over time." }
            div { class: "st-seg", role: "group", "aria-label": "Period",
                for r in StatsRange::ALL {
                    button {
                        class: if r == current { "st-seg-btn on" } else { "st-seg-btn" },
                        r#type: "button",
                        "aria-pressed": if r == current { "true" } else { "false" },
                        onclick: move |_| range.set(r),
                        {r.label()}
                    }
                }
            }
            button {
                class: "st-range-trigger",
                r#type: "button",
                "data-testid": "stats-range-trigger",
                onclick: move |_| sheet_open.set(true),
                {current.label()}
                span { class: "st-range-chevron", aria_hidden: "true", "\u{25BE}" }
            }
        }
    }
}

/// Mobile bottom sheet listing the four period options.
#[component]
fn RangeSheet(range: Signal<StatsRange>, sheet_open: Signal<bool>) -> Element {
    let current = range();
    rsx! {
        div {
            class: "m-sheet-scrim",
            "data-testid": "stats-range-sheet",
            onclick: move |_| sheet_open.set(false),
            div { class: "m-sheet", onclick: move |e| e.stop_propagation(),
                div { class: "m-sheet-grabber" }
                div { class: "m-sheet-head",
                    h4 { "Period" }
                }
                div { class: "m-sheet-body",
                    div { class: "st-range-rows",
                        for r in StatsRange::ALL {
                            button {
                                class: if r == current { "st-range-row on" } else { "st-range-row" },
                                r#type: "button",
                                onclick: move |_| {
                                    range.set(r);
                                    sheet_open.set(false);
                                },
                                {r.label()}
                            }
                        }
                    }
                }
            }
        }
    }
}

/// Period-scoped summary strip — scaffold the metric tiles replace next.
#[component]
fn PeriodSummary(period: Signal<Option<StatsSummary>>) -> Element {
    let guard = period.read();
    let Some(summary) = guard.as_ref() else {
        return rsx! { div { class: "card st-card-placeholder", aria_hidden: "true" } };
    };
    let sessions = summary.sessions;
    let time = format_active_time(summary.total_seconds());
    rsx! {
        div { class: "card st-summary-card", "data-testid": "stats-period-summary",
            div { class: "label", "This period" }
            div { class: "st-summary-line mono", "{sessions} sessions \u{00B7} {time}" }
        }
    }
}

/// All-time summary strip — scaffold the heatmap card replaces next.
#[component]
fn AllTimeSummary(all_time: Signal<Option<StatsSummary>>) -> Element {
    let guard = all_time.read();
    let Some(summary) = guard.as_ref() else {
        return rsx! { div { class: "card st-card-placeholder", aria_hidden: "true" } };
    };
    let active = summary.active_days;
    let streak = summary.longest_streak_days;
    rsx! {
        div { class: "card st-summary-card", "data-testid": "stats-alltime-summary",
            div { class: "label", "Reading days" }
            div { class: "st-summary-line mono",
                "{active} active days \u{00B7} {streak}-day longest streak"
            }
        }
    }
}

/// Friendly empty state for a user with no recorded activity.
#[component]
fn StatsEmpty() -> Element {
    rsx! {
        div { class: "card st-empty", "data-testid": "stats-empty",
            h3 { class: "st-empty-title", "No reading activity yet" }
            p { class: "st-empty-sub",
                "Open a book or start an audiobook and your stats will begin to fill in."
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn period_word_lowercases_labels_and_renders_all_time_as_lifetime() {
        assert_eq!(period_word(StatsRange::Week), "week");
        assert_eq!(period_word(StatsRange::Month), "month");
        assert_eq!(period_word(StatsRange::Year), "year");
        assert_eq!(period_word(StatsRange::AllTime), "lifetime");
    }

    #[test]
    fn format_active_time_covers_minute_hour_and_mixed_spans() {
        assert_eq!(format_active_time(0), "0 m");
        assert_eq!(format_active_time(59), "0 m");
        assert_eq!(format_active_time(42 * 60), "42 m");
        assert_eq!(format_active_time(3 * 3600), "3 h");
        assert_eq!(format_active_time(3 * 3600 + 20 * 60), "3 h 20 m");
    }
}
