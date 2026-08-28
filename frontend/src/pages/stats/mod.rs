//! Reading-stats page (`/stats`). Period-scoped modules up top driven by the
//! Week / Month / Year / Lifetime dropdown embedded in the page title, then an
//! explicitly separated All-time section that never re-queries on period
//! change. Mirrors the converged Stats design (`screens/stats-converged.jsx`).

use dioxus::prelude::*;
use omnibus_shared::{
    LibraryComposition, LibrarySize, ReadingGoal, StatsRange, StatsSummary, STATS_TTL_SECS,
};

use crate::components::{PageError, PageLoading, SessionLogList};
use crate::{data, use_server_url, Route};

mod composition;
mod donut;
mod drill_in;
mod goal;
mod heatmap;
mod library;
mod monthly;
mod patterns;
mod superlatives;
mod tiles;

use composition::LibraryCompositionCard;
use donut::{FormatSplit, GenreDonut, LengthSplit};
use drill_in::{DrillIn, Metric};
use goal::GoalBand;
use heatmap::HeatmapCard;
use library::LibrarySizeCard;
use monthly::MonthlyChart;
use patterns::TimePatternsCard;
use superlatives::SuperlativesCard;
use tiles::HeadlineTiles;

/// Group a non-negative integer's digits in threes with `,` separators.
/// Negative inputs (never produced by `db::stats`) fall back to a plain
/// `to_string` rather than mangling the sign.
fn group_thousands(n: i64) -> String {
    if n < 0 {
        return n.to_string();
    }
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

/// The italicized period word in the page title.
fn period_word(range: StatsRange) -> &'static str {
    match range {
        StatsRange::Week => "week",
        StatsRange::Month => "month",
        StatsRange::Year => "year",
        StatsRange::AllTime => "lifetime",
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
    // Library-scale rather than per-user, so it rides its own fetch: folding
    // it into the summary would recompute and re-send it on every switcher
    // change. `None` until it lands, and the card renders nothing then.
    let library_size: Signal<Option<LibrarySize>> = use_signal(|| None);
    // Same reasoning, same shape: what the library is *made of* is a
    // library-wide answer that only moves on a reindex.
    let library_composition: Signal<Option<LibraryComposition>> = use_signal(|| None);
    let loading = use_signal(|| true);
    let error: Signal<Option<String>> = use_signal(|| None);
    // Seeded closed on every target (rule 07): the title dropdown only ever
    // opens from a client click.
    let menu_open = use_signal(|| false);
    // Which tile's drill-in is open, if any — seeded closed on every target
    // (rule 07): the sheet/modal only ever opens from a client click.
    let expanded: Signal<Option<Metric>> = use_signal(|| None);
    // The current-year goal, owned by the page rather than read straight off
    // the summary: a save writes the server's answer back here, so the band
    // updates without waiting on a refetch.
    let goal: Signal<Option<ReadingGoal>> = use_signal(|| None);

    use_period_fetch_effect(server_url.clone(), range, period, error);
    use_all_time_fetch_effect(server_url.clone(), all_time, loading, error, goal);
    use_library_size_fetch_effect(server_url.clone(), library_size);
    use_library_composition_fetch_effect(server_url.clone(), library_composition);

    if loading() {
        return rsx! { PageLoading {} };
    }
    if let Some(msg) = error() {
        return rsx! { PageError { message: msg, back_to: Route::Landing {} } };
    }

    let empty = all_time.read().as_ref().is_none_or(StatsSummary::is_empty);

    // The goal is annual, so it is anchored *outside* the period-scoped
    // section and sourced from the all-time summary — the one fetch that never
    // re-runs on a range change. It sits under the masthead rather than over
    // it so the page still opens with its `h1`, and so the title's period
    // dropdown keeps opening into the same space (a band above it pushes the
    // menu down over the page's midpoint).
    let goal_year = goal_year_from(all_time.read().as_ref());

    rsx! {
        div { class: "st-page",
            StatsHeader { range, menu_open }
            if let Some(year) = goal_year {
                GoalBand { goal, year, server_url: server_url.clone() }
            }
            if empty {
                StatsEmpty {}
            } else {
                // Deliberately not keyed on the range: a period switch keeps
                // the cards mounted (the entrance cascade plays on page load
                // only) and each card's *contents* transition when the new
                // summary lands — the web analogue of iOS numericText.
                section { class: "st-period", "data-testid": "stats-period-section",
                    PeriodSummary { period, expanded }
                }
                div { class: "st-divider",
                    h3 { class: "st-divider-title", "All" span { class: "st-divider-em", "-time" } }
                    p { class: "st-divider-sub", "Not tied to the period above." }
                }
                section { class: "st-alltime", "data-testid": "stats-alltime-section",
                    AllTimeSummary { all_time }
                    LibrarySizeCard { size: library_size() }
                    LibraryCompositionCard { composition: library_composition() }
                }
                // The log sits under the aggregates it details, and outside
                // the period switcher's reach: it is its own paged read, not
                // a window rollup.
                SessionLogList { book: None, compact: false }
            }
            if let (Some(metric), Some(summary)) = (expanded(), period.read().clone()) {
                DrillIn { metric, summary, range: range(), expanded }
            }
            StatsFreshnessNote {}
        }
    }
}

/// The calendar year the goal band labels itself with, taken from the
/// server's `as_of_day` (`YYYY-MM-DD`) rather than a client clock — the band
/// would otherwise disagree with the server across a New Year's Eve timezone
/// gap, and a client-derived year in markup is a hydration hazard (rule 07).
/// `None` until the summary lands, or if it carries no `as_of_day`.
fn goal_year_from(summary: Option<&StatsSummary>) -> Option<String> {
    // `get` rather than an index: a malformed non-ASCII value would panic on
    // a char boundary, and a stats page must not die over a label.
    Some(summary?.as_of_day.get(..4)?.to_string())
}

/// The footer note's text, deriving its number from [`STATS_TTL_SECS`]
/// rather than a second hardcoded copy.
fn freshness_note_text() -> String {
    format!("Stats are accurate to the last ~{STATS_TTL_SECS} seconds.")
}

/// Footer note explaining the aggregate cache's TTL, so a just-made change
/// (e.g. marking a book finished) not yet reflected in the numbers reads as
/// expected staleness rather than a bug.
#[component]
fn StatsFreshnessNote() -> Element {
    rsx! {
        p { class: "st-footnote", "data-testid": "stats-freshness-note",
            {freshness_note_text()}
        }
    }
}

/// Refetch the period-scoped summary whenever the switcher changes. The
/// signal read inside the effect subscribes it to `range`.
///
/// A monotonic `epoch` ticket guards against out-of-order completion: rapid
/// switcher changes fan out concurrent fetches, and a slower earlier request
/// must not overwrite a newer range's data. Only the fetch holding the current
/// ticket applies its result, and a success clears any prior error so a
/// transient failure can't stick the page in the error state.
fn use_period_fetch_effect(
    server_url: String,
    range: Signal<StatsRange>,
    period: Signal<Option<StatsSummary>>,
    error: Signal<Option<String>>,
) {
    let mut epoch = use_signal(|| 0u64);
    let generation = crate::use_cache_generation();
    use_effect(move || {
        let r = range();
        // Re-run on cache-revalidation bumps; the refetch is a cache hit.
        let _ = generation();
        let ticket = *epoch.peek() + 1;
        epoch.set(ticket);
        let url = server_url.clone();
        let mut period = period;
        let mut error = error;
        spawn(async move {
            let result = data::fetch_stats(&url, r).await;
            // A newer switcher change superseded this fetch — drop the stale result.
            if *epoch.peek() != ticket {
                return;
            }
            match result {
                Ok(summary) => {
                    period.set(Some(summary));
                    error.set(None);
                }
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
    goal: Signal<Option<ReadingGoal>>,
) {
    let generation = crate::use_cache_generation();
    use_effect(move || {
        // Re-run on cache-revalidation bumps; the refetch is a cache hit.
        let _ = generation();
        let url = server_url.clone();
        let mut all_time = all_time;
        let mut loading = loading;
        let mut error = error;
        let mut goal = goal;
        spawn(async move {
            match data::fetch_stats(&url, StatsRange::AllTime).await {
                Ok(summary) => {
                    goal.set(summary.goal.clone());
                    all_time.set(Some(summary));
                }
                Err(e) => error.set(Some(e.to_string())),
            }
            loading.set(false);
        });
    });
}

/// One-shot fetch of the library-scale totals. Never keyed on the switcher —
/// the library's size is not a reporting period's figure — and deliberately
/// silent on failure: this card is context beside the reader's own numbers,
/// and a library-size fetch that fails must not blank the page they came for.
fn use_library_size_fetch_effect(server_url: String, library_size: Signal<Option<LibrarySize>>) {
    let generation = crate::use_cache_generation();
    use_effect(move || {
        // Re-run on cache-revalidation bumps; the refetch is a cache hit.
        let _ = generation();
        let url = server_url.clone();
        let mut library_size = library_size;
        spawn(async move {
            if let Ok(size) = data::fetch_library_size(&url).await {
                library_size.set(Some(size));
            }
        });
    });
}

/// One-shot fetch of the library composition. Never keyed on the switcher —
/// what the collection is made of is not a reporting period's figure — and
/// silent on failure for the same reason its size sibling is: this card is
/// context beside the reader's own numbers, and a failed fetch must not blank
/// the page they came for.
fn use_library_composition_fetch_effect(
    server_url: String,
    library_composition: Signal<Option<LibraryComposition>>,
) {
    let generation = crate::use_cache_generation();
    use_effect(move || {
        // Re-run on cache-revalidation bumps; the refetch is a cache hit.
        let _ = generation();
        let url = server_url.clone();
        let mut library_composition = library_composition;
        spawn(async move {
            if let Ok(composition) = data::fetch_library_composition(&url).await {
                library_composition.set(Some(composition));
            }
        });
    });
}

/// Editorial header: the title's italic period word doubles as the period
/// switcher — a trigger that drops the Week / Month / Year / Lifetime menu
/// straight out of the headline, one control across every form factor.
#[component]
fn StatsHeader(range: Signal<StatsRange>, menu_open: Signal<bool>) -> Element {
    let current = range();
    rsx! {
        header { class: "st-head",
            h1 { class: "st-title",
                "Your reading "
                span { class: "st-period-picker",
                    button {
                        class: "st-period-trigger",
                        r#type: "button",
                        "data-testid": "stats-range-trigger",
                        "aria-haspopup": "dialog",
                        "aria-expanded": "{menu_open()}",
                        "aria-describedby": "st-period-hint",
                        onclick: move |_| {
                            let next = !menu_open();
                            menu_open.set(next);
                        },
                        span { class: "st-period-word", {period_word(current)} }
                        span { class: "st-period-chevron", aria_hidden: "true", "\u{25BE}" }
                    }
                    if menu_open() {
                        div {
                            class: "st-period-scrim",
                            "data-testid": "stats-range-scrim",
                            onclick: move |_| menu_open.set(false),
                        }
                        RangeMenu { range, menu_open }
                    }
                }
            }
            // aria-describedby target, outside the h1 so the hint text never
            // joins the heading's accessible name ("Your reading month", not
            // "Your reading Change period month"). An aria-label on the
            // trigger would replace the period word as the button's name and
            // distort the heading the same way — a description adds the
            // action without touching either name.
            span { id: "st-period-hint", class: "sr-only", "Change period" }
            p { class: "st-sub", "Reading & listening, tracked over time." }
        }
    }
}

/// The open period dropdown, anchored under the title's period word.
///
/// `role="dialog"` (not `role="menu"`) to match the user-menu / export
/// dropdowns: a scrim-dismissed popover with plain buttons, not an ARIA menu
/// with roving-tabindex navigation.
#[component]
fn RangeMenu(range: Signal<StatsRange>, menu_open: Signal<bool>) -> Element {
    let current = range();
    let mut menu_open = menu_open;
    let on_keydown = move |evt: Event<KeyboardData>| {
        if evt.key() == Key::Escape {
            evt.prevent_default();
            menu_open.set(false);
        }
    };
    rsx! {
        div {
            class: "st-period-menu",
            role: "dialog",
            "aria-label": "Period",
            "data-testid": "stats-range-menu",
            tabindex: "-1",
            onkeydown: on_keydown,
            onmounted: move |evt: MountedEvent| focus_range_menu(&evt),
            for r in StatsRange::ALL {
                button {
                    key: "{r.as_query()}",
                    class: if r == current { "st-range-row on" } else { "st-range-row" },
                    r#type: "button",
                    "aria-pressed": if r == current { "true" } else { "false" },
                    onclick: move |_| {
                        range.set(r);
                        menu_open.set(false);
                    },
                    span { class: "st-range-row-label", {r.label()} }
                    if r == current {
                        span { class: "st-range-check", aria_hidden: "true", "\u{2713}" }
                    }
                }
            }
        }
    }
}

/// Focus the menu after paint so its `onkeydown` receives ESC — same
/// requestAnimationFrame timing as the user-menu and export panels. Web-only;
/// SSR never paints the menu, so the gate lives at the fn definition to keep
/// the `onmounted` handler body identical across targets (rule 07).
#[cfg(feature = "web")]
fn focus_range_menu(evt: &MountedEvent) {
    use dioxus::web::WebEventExt;
    use wasm_bindgen::prelude::*;

    let Some(element) = evt.try_as_web_event() else {
        return;
    };
    let Some(window) = web_sys::window() else {
        return;
    };
    let cb = Closure::once_into_js(move || {
        if let Some(html_el) = element.dyn_ref::<web_sys::HtmlElement>() {
            let _ = html_el.focus();
        }
    });
    let _ = window.request_animation_frame(cb.unchecked_ref());
}

/// Non-web stub (SSR): nothing to focus before hydration.
#[cfg(not(feature = "web"))]
fn focus_range_menu(_evt: &MountedEvent) {}

/// The period-scoped module stack: headline tiles, the composition row (genre
/// donut + format split, with the length distribution beneath them), then the
/// superlatives card. A placeholder card until the first fetch lands. `expanded` is forwarded to the
/// tiles so a grip click can open that metric's drill-in.
#[component]
fn PeriodSummary(
    period: Signal<Option<StatsSummary>>,
    expanded: Signal<Option<Metric>>,
) -> Element {
    let guard = period.read();
    let Some(summary) = guard.as_ref() else {
        return rsx! { div { class: "card st-card-placeholder", aria_hidden: "true" } };
    };
    rsx! {
        HeadlineTiles {
            books_finished: summary.books_finished,
            avg_stars: summary.avg_stars,
            pages_read: summary.pages_read,
            pages_audio_only: summary.pages_detail.audio_only(),
            listening_seconds: summary.listening_seconds,
            expanded,
        }
        div { class: "st-compose",
            GenreDonut { summary: summary.clone() }
            FormatSplit { summary: summary.clone() }
            LengthSplit { summary: summary.clone() }
        }
        TimePatternsCard { summary: summary.clone() }
        SuperlativesCard { summary: summary.clone() }
    }
}

/// The all-time module stack: the reading-days heatmap card (with the
/// longest-streak figure in its header), then the books-per-month trend
/// chart. The library-size and library-composition cards sit alongside these
/// in the same section but ride their own fetches, so the page mounts them
/// rather than this component.
#[component]
fn AllTimeSummary(all_time: Signal<Option<StatsSummary>>) -> Element {
    let guard = all_time.read();
    let Some(summary) = guard.as_ref() else {
        return rsx! { div { class: "card st-card-placeholder", aria_hidden: "true" } };
    };
    rsx! {
        HeatmapCard { summary: summary.clone() }
        MonthlyChart { summary: summary.clone() }
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
    fn goal_year_from_takes_the_year_off_the_servers_as_of_day() {
        let mut summary = StatsSummary {
            as_of_day: "2026-08-28".to_string(),
            ..StatsSummary::default()
        };
        assert_eq!(goal_year_from(Some(&summary)), Some("2026".to_string()));

        // No summary yet, and a summary from a server too old to send the day.
        assert_eq!(goal_year_from(None), None);
        summary.as_of_day = String::new();
        assert_eq!(goal_year_from(Some(&summary)), None);
    }

    #[test]
    fn freshness_note_text_states_the_real_ttl_in_seconds() {
        assert_eq!(
            freshness_note_text(),
            format!("Stats are accurate to the last ~{STATS_TTL_SECS} seconds.")
        );
    }

    #[test]
    fn group_thousands_handles_short_and_negative_inputs() {
        assert_eq!(group_thousands(0), "0");
        assert_eq!(group_thousands(9), "9");
        assert_eq!(group_thousands(999), "999");
        assert_eq!(group_thousands(1000), "1,000");
        assert_eq!(group_thousands(1_234_567), "1,234,567");
        assert_eq!(group_thousands(-42), "-42");
    }
}
