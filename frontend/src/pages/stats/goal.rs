//! The goal cluster in the stats hero: the annual books ring and the standing
//! daily pages and minutes rows. Anchored to the calendar year and to today,
//! so it sits outside the windowed band and never moves when the period
//! switcher does.
//!
//! **Read-only.** Goals are account configuration, and all three are set
//! together in Settings → Account (`pages::account::goals`). This surface
//! reports them; it does not edit them.

use dioxus::prelude::*;
use dioxus_router::Link;
use omnibus_shared::{DailyGoal, DailyGoals, ReadingGoal};

use super::heatmap::{civil_from_days, day_number, days_from_civil};
use crate::Route;

/// Pluralize a unit noun. The captions read as sentences, so "1 book" has to
/// win over a parenthesised "(s)".
fn plural(unit: &str, n: i64) -> String {
    if n == 1 {
        unit.to_string()
    } else {
        format!("{unit}s")
    }
}

/// Progress caption — the honest ratio, not the clamped percentage, so a
/// reader past their target sees that they are.
///
/// Shared by the annual ring and both daily rows: they differ only in their
/// unit, and a second copy of this would be a second place for the
/// past-the-target rule to be got wrong.
fn progress_caption(current: i64, target: i64, unit: &str) -> String {
    format!("{current} of {target} {}", plural(unit, target))
}

/// Trailing status line: what is left, or that the goal is met.
fn remainder_caption(current: i64, target: i64, unit: &str) -> String {
    if current >= target {
        return "Goal met".to_string();
    }
    let left = target - current;
    format!("{left} {} to go", plural(unit, left))
}

/// `aria-valuenow` for a progress bar. ARIA requires it inside
/// `[aria-valuemin, aria-valuemax]`, so a reader past their target has to be
/// clamped here — the real, unclamped ratio is announced through
/// `aria-valuetext` instead, so nothing is hidden from a screen reader.
fn aria_value_now(current: i64, target: i64) -> i64 {
    current.clamp(0, target)
}

/// How many whole minutes `secs` is, for the unplaceable-sessions disclosure.
/// Truncating, to match the way the goal itself counts a partial minute.
fn disclosure_minutes(secs: i64) -> i64 {
    secs / 60
}

/// How far through its year `day` is, as a fraction 0.0..=1.0 — the pace an
/// annual target is read against.
///
/// Taken off the server's own `as_of_day` rather than a client clock, so the
/// pace note can't disagree with the figure beside it across a timezone gap,
/// and so no date reaches the markup that SSR and the first WASM paint could
/// derive differently (rule 07). `None` for a day the server never sent.
pub(super) fn year_fraction(as_of_day: &str) -> Option<f64> {
    let n = day_number(as_of_day)?;
    let (y, _, _) = civil_from_days(n);
    let start = days_from_civil(y, 1, 1);
    let length = days_from_civil(y + 1, 1, 1) - start;
    if length <= 0 {
        return None;
    }
    // Day-of-year over year length: both are small integers, exact in f64.
    #[allow(clippy::cast_precision_loss)]
    let fraction = (n - start + 1) as f64 / length as f64;
    Some(fraction)
}

/// "3 ahead of pace" / "2 behind pace" / "on pace" — where the reader stands
/// against an even spread of the target across the year.
///
/// `None` when the goal is already met (the ring says so on its own) or when
/// the server sent no day to measure the year against.
fn pace_note(goal: &ReadingGoal, as_of_day: &str) -> Option<String> {
    if goal.is_met() {
        return None;
    }
    let fraction = year_fraction(as_of_day)?;
    // Targets are bounded at MAX_GOAL_TARGET, far inside f64's exact range.
    #[allow(clippy::cast_precision_loss)]
    let expected = goal.target as f64 * fraction;
    #[allow(clippy::cast_possible_truncation)]
    let diff = (goal.current as f64 - expected).round() as i64;
    Some(match diff {
        0 => "on pace".to_string(),
        d if d > 0 => format!("{d} ahead of pace"),
        d => format!("{} behind pace", -d),
    })
}

/// The link every "no goal set" state offers — one destination, because there
/// is one place goals are set.
#[component]
fn SetGoalsLink(label: &'static str, testid: &'static str) -> Element {
    rsx! {
        Link {
            class: "st-goal-link",
            "data-testid": testid,
            to: Route::Settings { section: None },
            {label}
        }
    }
}

/// The annual books goal: a conic ring carrying the ratio, with the figure and
/// pace beside it.
///
/// With no target it drops the ring and reports `finished` — the year's real
/// count — under a "so far" qualifier, the same trade the daily rows make: the
/// figure is worth showing before a reader commits to a target, but the ring
/// is a claim only a target can support.
///
/// `year` and `as_of_day` are both the server's, taken from the summary rather
/// than a client clock (rule 07).
#[component]
pub(super) fn AnnualGoalRing(
    goal: Option<ReadingGoal>,
    finished: Option<i64>,
    year: String,
    as_of_day: String,
) -> Element {
    // The kicker carries the timeframe rather than repeating it under the
    // figure: "This year" over "22 books" over "2026 so far" said the same
    // thing twice. With a target the ring's own caption names the year, so the
    // kicker only has to say which span it describes.
    let kicker = if goal.is_some() {
        "This year".to_string()
    } else {
        format!("{year} so far")
    };
    rsx! {
        div { class: "st-year", "data-testid": "stats-goal",
            // A ring is a claim about a target, so an unset goal draws none —
            // an empty circle beside "No goal set" reads as a goal of zero.
            if let Some(g) = goal.clone() {
                div {
                    class: "st-year-ring",
                    style: "--st-ring-pct: {g.percent()}%",
                    "data-testid": "stats-goal-progress",
                    role: "progressbar",
                    "aria-valuemin": "0",
                    "aria-valuemax": "{g.target}",
                    "aria-valuenow": "{aria_value_now(g.current, g.target)}",
                    "aria-valuetext": "{progress_caption(g.current, g.target, \"book\")}",
                    "aria-label": "Books finished in {g.year}",
                    div { class: "st-year-disc",
                        div { class: "st-year-figure", "data-testid": "stats-goal-ring",
                            "{g.current}"
                            span { class: "st-year-target", "/{g.target}" }
                        }
                        div { class: "st-year-caption", "{year} goal" }
                    }
                }
            }
            div { class: "st-year-side",
                span { class: "st-year-kicker", {kicker.clone()} }
                if let Some(g) = goal.clone() {
                    // The honest ratio, where the ring carries the clamped
                    // arc: a reader past their target sees that they are.
                    p { class: "st-year-line", "data-testid": "stats-goal-figure",
                        {progress_caption(g.current, g.target, "book")}
                    }
                    p { class: "st-year-note", "data-testid": "stats-goal-note",
                        {remainder_caption(g.current, g.target, "book")}
                        if let Some(pace) = pace_note(&g, &as_of_day) {
                            span { class: "st-year-pace", " \u{00B7} {pace}" }
                        }
                    }
                } else if let Some(n) = finished {
                    div { class: "st-year-bare",
                        p { class: "st-year-figure bare", "data-testid": "stats-goal-today",
                            "{n} "
                            span { class: "st-year-bare-unit", {plural("book", n)} }
                        }
                    }
                    SetGoalsLink { label: "Set a goal", testid: "stats-goal-set-link" }
                } else {
                    p { class: "st-year-invite", "data-testid": "stats-goal-invite",
                        "No target for the year yet."
                    }
                    SetGoalsLink { label: "Set a goal", testid: "stats-goal-set-link" }
                }
            }
        }
    }
}

/// The standing daily goals card: the pages and minutes rows, the goals-met
/// chip, and the unplaceable-sessions disclosure beneath them.
#[component]
pub(super) fn DailyGoalsCard(daily: DailyGoals) -> Element {
    // "Every day" promises a recurrence, which is only true once something
    // recurs; with no target at all the rows are just what happened today.
    let neither = daily.pages.is_none() && daily.minutes.is_none();
    rsx! {
        div { class: "st-daily", "data-testid": "stats-daily-goals",
            div { class: "st-daily-head",
                // No card-level "Goals met" badge: each row already says so on
                // its own, and a third copy of the same fact in the corner made
                // a met day read as three separate announcements.
                span { class: "st-daily-eyebrow",
                    if neither { "Today" } else { "Every day" }
                }
            }
            DailyGoalRow {
                kind: "pages",
                noun: "Pages",
                unit: "page",
                goal: daily.pages.clone(),
                today: daily.pages_today,
                alone: neither,
            }
            DailyGoalRow {
                kind: "minutes",
                noun: "Minutes",
                unit: "minute",
                goal: daily.minutes.clone(),
                today: daily.minutes_today,
                alone: neither,
            }
            // One short call to action while either kind is still unset, at the
            // card's foot rather than in each row: there is one place to set
            // them, so there is one thing to click.
            if daily.pages.is_none() || daily.minutes.is_none() {
                SetGoalsLink { label: "Set goals", testid: "stats-daily-set-link" }
            }
            // Only ever non-zero alongside a minutes goal, and it is a
            // disclosure rather than an error: those seconds are real reading
            // the goal could not place on a day.
            if daily.unzoned_seconds > 0 {
                p { class: "st-daily-unzoned", "data-testid": "stats-daily-unzoned",
                    {
                        let mins = disclosure_minutes(daily.unzoned_seconds);
                        format!(
                            "{mins} {} today came from sessions that recorded no time zone, \
                             so they aren't counted above.",
                            plural("minute", mins),
                        )
                    }
                }
            }
        }
    }
}

/// The label for one row: the noun, plus whatever timeframe the row itself has
/// to supply.
///
/// A row with a target says "a day" — that is what the target means. Without
/// one the noun stands alone when the card's own header already says "Today",
/// and carries "today" itself when the header has moved on to "Every day" for
/// the sake of a sibling row that *is* set. The word appears exactly once
/// either way.
fn row_label(noun: &str, has_target: bool, alone: bool) -> String {
    match (has_target, alone) {
        (true, _) => format!("{noun} a day"),
        (false, true) => noun.to_string(),
        (false, false) => format!("{noun} today"),
    }
}

/// One daily goal's row.
///
/// With a target it draws the ratio and the bar. Without one it still draws
/// today's figure — a bare count, since [`row_label`] has already named what is
/// being counted — so a reader can see what they have done before deciding what
/// to aim for. That mirrors the iOS card, where an untargeted kind keeps its
/// slot and drops only the ring: **a bar, like a ring, is a claim about a
/// target.**
#[component]
fn DailyGoalRow(
    kind: String,
    noun: String,
    unit: String,
    goal: Option<DailyGoal>,
    today: Option<i64>,
    alone: bool,
) -> Element {
    let label = row_label(&noun, goal.is_some(), alone);
    rsx! {
        div { class: "st-daily-row", "data-testid": "stats-daily-{kind}",
            div { class: "st-daily-top",
                span { class: "st-daily-label", "{label}" }
            }
            if let Some(g) = goal.clone() {
                div { class: "st-daily-figures",
                    p { class: "st-daily-figure", "data-testid": "stats-daily-{kind}-figure",
                        {progress_caption(g.current, g.target, &unit)}
                    }
                    p {
                        class: if g.is_met() { "st-daily-note met" } else { "st-daily-note" },
                        "data-testid": "stats-daily-{kind}-note",
                        {remainder_caption(g.current, g.target, &unit)}
                    }
                }
                div {
                    class: "st-goal-track",
                    "data-testid": "stats-daily-{kind}-progress",
                    role: "progressbar",
                    "aria-valuemin": "0",
                    "aria-valuemax": "{g.target}",
                    "aria-valuenow": "{aria_value_now(g.current, g.target)}",
                    "aria-valuetext": "{progress_caption(g.current, g.target, &unit)}",
                    "aria-label": "{label} on {g.day}",
                    div {
                        class: if g.is_met() { "st-goal-fill met" } else { "st-goal-fill" },
                        style: "width: {g.percent()}%",
                    }
                }
            } else if let Some(n) = today {
                p { class: "st-daily-figure bare", "data-testid": "stats-daily-{kind}-today",
                    "{n}"
                }
            } else {
                p { class: "st-daily-invite", "data-testid": "stats-daily-{kind}-invite",
                    "No target set."
                }
            }
        }
    }
}

#[cfg(test)]
mod tests;
