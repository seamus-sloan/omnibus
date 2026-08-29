//! The reading-goal band at the top of `/stats` — the annual books target, the
//! standing daily pages and minutes targets beneath it, and the inline editors
//! behind each. Anchored to the calendar year, so it sits outside the
//! period-scoped section (directly under the masthead) and never moves when the
//! switcher does.

use dioxus::prelude::*;
use omnibus_shared::{
    DailyGoal, DailyGoalUpdate, DailyGoals, ReadingGoal, ReadingGoalUpdate, GOAL_KIND_MINUTES,
    GOAL_KIND_PAGES, MAX_DAILY_MINUTES, MAX_DAILY_PAGES, MAX_GOAL_TARGET,
};

use crate::data;

/// True when the shell's connectivity tracker says the server is unreachable.
///
/// The cfg lives on the function, never inside a component body (rule 07): web
/// and SSR both take the constant-`false` arm, so the first client paint and
/// the server render agree on whether the control is disabled.
#[cfg(feature = "mobile")]
fn is_offline() -> bool {
    crate::offline::sync::is_offline()
}

/// Web/SSR: the browser client has no offline replica, so the control is
/// always live and a failed save surfaces its own error.
#[cfg(not(feature = "mobile"))]
fn is_offline() -> bool {
    false
}

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
/// Shared by the annual band and both daily rows: they differ only in their
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

/// Parse and bound an editor's draft, returning the message the field should
/// show on rejection. Mirrors the write path's own bounds so a typo never costs
/// a round trip.
fn parse_target(draft: &str, unit: &str, max: i64) -> Result<i64, String> {
    let trimmed = draft.trim();
    if trimmed.is_empty() {
        return Err(format!("Enter a number of {}.", plural(unit, 2)));
    }
    let Ok(target) = trimmed.parse::<i64>() else {
        return Err(format!("Enter a whole number of {}.", plural(unit, 2)));
    };
    if !(1..=max).contains(&target) {
        return Err(format!("Pick a target between 1 and {max}."));
    }
    Ok(target)
}

/// How many whole minutes `secs` is, for the unplaceable-sessions disclosure.
/// Truncating, to match the way the goal itself counts a partial minute.
fn disclosure_minutes(secs: i64) -> i64 {
    secs / 60
}

/// Fire one annual goal write and fold the server's answer back into the band.
///
/// A free function rather than a closure over the signals: two controls (Save
/// and Clear) call it, and a closure capturing `Signal::set` isn't `Copy`, so
/// the second `onclick` would move a value the first already took.
fn submit(
    server_url: String,
    update: ReadingGoalUpdate,
    mut goal: Signal<Option<ReadingGoal>>,
    mut editing: Signal<bool>,
    mut saving: Signal<bool>,
    mut error: Signal<Option<String>>,
) {
    saving.set(true);
    spawn(async move {
        match data::save_reading_goal(&server_url, &update).await {
            Ok(next) => {
                goal.set(next);
                editing.set(false);
                error.set(None);
            }
            Err(e) => error.set(Some(e.to_string())),
        }
        saving.set(false);
    });
}

/// The daily counterpart of [`submit`]. The server answers with **both** daily
/// goals, so one write refreshes the whole set rather than leaving the other
/// row showing progress measured against an older read.
fn submit_daily(
    server_url: String,
    update: DailyGoalUpdate,
    mut daily: Signal<DailyGoals>,
    mut editing: Signal<bool>,
    mut saving: Signal<bool>,
    mut error: Signal<Option<String>>,
) {
    saving.set(true);
    spawn(async move {
        match data::save_daily_goal(&server_url, &update).await {
            Ok(next) => {
                daily.set(next);
                editing.set(false);
                error.set(None);
            }
            Err(e) => error.set(Some(e.to_string())),
        }
        saving.set(false);
    });
}

/// The goal band. `goal` and `daily` are the page's current goals (owned there
/// so a save updates them in place rather than waiting on a refetch), and
/// `year` is the server's calendar year, taken from `StatsSummary::as_of_day`
/// so no client clock reaches the markup.
#[component]
pub fn GoalBand(
    goal: Signal<Option<ReadingGoal>>,
    daily: Signal<DailyGoals>,
    year: String,
    server_url: String,
) -> Element {
    // All three seeded to the same value on every target (rule 07): the editor
    // only ever opens from a client click.
    let mut editing = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let saving = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let offline = is_offline();
    let current = goal.read().clone();
    let dailies = daily.read().clone();

    let save_url = server_url.clone();
    let clear_url = server_url.clone();

    let open_editor = {
        let current = current.clone();
        move |_| {
            draft.set(
                current
                    .as_ref()
                    .map_or_else(String::new, |g| g.target.to_string()),
            );
            error.set(None);
            editing.set(true);
        }
    };

    rsx! {
        section {
            class: "card st-goal",
            "data-testid": "stats-goal",
            "aria-label": "Reading goal",
            div { class: "st-goal-top",
                span { class: "st-goal-eyebrow", "{year} reading goal" }
                if !editing() {
                    button {
                        class: "st-goal-trigger",
                        r#type: "button",
                        "data-testid": "stats-goal-edit",
                        disabled: offline,
                        onclick: open_editor,
                        if current.is_some() { "Edit" } else { "Set a goal" }
                    }
                }
            }

            if let Some(g) = current.clone() {
                p { class: "st-goal-figure", "data-testid": "stats-goal-figure",
                    {progress_caption(g.current, g.target, "book")}
                }
                div {
                    class: "st-goal-track",
                    "data-testid": "stats-goal-progress",
                    role: "progressbar",
                    "aria-valuemin": "0",
                    "aria-valuemax": "{g.target}",
                    "aria-valuenow": "{aria_value_now(g.current, g.target)}",
                    "aria-valuetext": "{progress_caption(g.current, g.target, \"book\")}",
                    "aria-label": "Books finished in {g.year}",
                    div {
                        class: if g.is_met() { "st-goal-fill met" } else { "st-goal-fill" },
                        style: "width: {g.percent()}%",
                    }
                }
                p { class: "st-goal-note", "data-testid": "stats-goal-note",
                    {remainder_caption(g.current, g.target, "book")}
                }
            } else {
                p { class: "st-goal-invite", "data-testid": "stats-goal-invite",
                    "Set a target for how many books you'd like to finish this year, and this
                     band will track it."
                }
            }

            if editing() {
                div { class: "st-goal-editor", "data-testid": "stats-goal-editor",
                    label { class: "st-goal-label", r#for: "st-goal-target", "Books this year" }
                    input {
                        class: "st-goal-input",
                        id: "st-goal-target",
                        "data-testid": "stats-goal-input",
                        r#type: "number",
                        min: "1",
                        max: "{MAX_GOAL_TARGET}",
                        inputmode: "numeric",
                        value: "{draft()}",
                        disabled: saving(),
                        oninput: move |evt| draft.set(evt.value()),
                    }
                    div { class: "st-goal-actions",
                        button {
                            class: "btn primary",
                            r#type: "button",
                            "data-testid": "stats-goal-save",
                            disabled: saving() || offline,
                            onclick: move |_| {
                                match parse_target(&draft(), "book", MAX_GOAL_TARGET) {
                                    Ok(target) => submit(
                                        save_url.clone(),
                                        ReadingGoalUpdate::books(target),
                                        goal,
                                        editing,
                                        saving,
                                        error,
                                    ),
                                    Err(msg) => error.set(Some(msg)),
                                }
                            },
                            "Save"
                        }
                        if current.is_some() {
                            button {
                                class: "btn",
                                r#type: "button",
                                "data-testid": "stats-goal-clear",
                                disabled: saving() || offline,
                                onclick: move |_| submit(
                                    clear_url.clone(),
                                    ReadingGoalUpdate::clear_books(),
                                    goal,
                                    editing,
                                    saving,
                                    error,
                                ),
                                "Clear"
                            }
                        }
                        button {
                            class: "btn",
                            r#type: "button",
                            "data-testid": "stats-goal-cancel",
                            disabled: saving(),
                            onclick: move |_| {
                                editing.set(false);
                                error.set(None);
                            },
                            "Cancel"
                        }
                    }
                    if offline {
                        p { class: "st-goal-offline", "data-testid": "stats-goal-offline",
                            "You're offline — a goal is account settings, so it can't be queued."
                        }
                    }
                }
            }

            if let Some(msg) = error() {
                p { class: "st-goal-error", role: "alert", "data-testid": "stats-goal-error",
                    {msg}
                }
            }

            div { class: "st-daily", "data-testid": "stats-daily-goals",
                span { class: "st-daily-eyebrow", "Every day" }
                DailyGoalRow {
                    kind: GOAL_KIND_PAGES,
                    label: "Pages a day",
                    unit: "page",
                    max: MAX_DAILY_PAGES,
                    goal: dailies.pages.clone(),
                    daily,
                    server_url: server_url.clone(),
                }
                DailyGoalRow {
                    kind: GOAL_KIND_MINUTES,
                    label: "Minutes a day",
                    unit: "minute",
                    max: MAX_DAILY_MINUTES,
                    goal: dailies.minutes.clone(),
                    daily,
                    server_url,
                }
                // Only ever non-zero alongside a minutes goal, and it is a
                // disclosure rather than an error: those seconds are real
                // reading the goal could not place on a day.
                if dailies.unzoned_seconds > 0 {
                    p { class: "st-daily-unzoned", "data-testid": "stats-daily-unzoned",
                        {
                            let mins = disclosure_minutes(dailies.unzoned_seconds);
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
}

/// One daily goal's row: its progress, and the inline editor behind it.
///
/// A component per kind rather than one editor over both, because the write
/// path is per-kind: a reader changing their pages target has said nothing
/// about their minutes target, and a combined Save would have to either send
/// two requests or silently restate a value nobody touched.
#[component]
fn DailyGoalRow(
    kind: String,
    label: String,
    unit: String,
    max: i64,
    goal: Option<DailyGoal>,
    daily: Signal<DailyGoals>,
    server_url: String,
) -> Element {
    let mut editing = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let saving = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let offline = is_offline();
    let field_id = format!("st-daily-{kind}");
    let save_url = server_url.clone();
    let clear_url = server_url;
    let save_kind = kind.clone();
    let clear_kind = kind.clone();
    let save_unit = unit.clone();

    let open_editor = {
        let goal = goal.clone();
        move |_| {
            draft.set(
                goal.as_ref()
                    .map_or_else(String::new, |g| g.target.to_string()),
            );
            error.set(None);
            editing.set(true);
        }
    };

    rsx! {
        div { class: "st-daily-row", "data-testid": "stats-daily-{kind}",
            div { class: "st-daily-top",
                span { class: "st-daily-label", "{label}" }
                if !editing() {
                    button {
                        class: "st-goal-trigger",
                        r#type: "button",
                        "data-testid": "stats-daily-{kind}-edit",
                        disabled: offline,
                        onclick: open_editor,
                        if goal.is_some() { "Edit" } else { "Set" }
                    }
                }
            }

            if let Some(g) = goal.clone() {
                p { class: "st-daily-figure", "data-testid": "stats-daily-{kind}-figure",
                    {progress_caption(g.current, g.target, &unit)}
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
                p { class: "st-goal-note", "data-testid": "stats-daily-{kind}-note",
                    {remainder_caption(g.current, g.target, &unit)}
                }
            } else {
                p { class: "st-daily-invite", "data-testid": "stats-daily-{kind}-invite",
                    "No daily target set."
                }
            }

            if editing() {
                div { class: "st-goal-editor", "data-testid": "stats-daily-{kind}-editor",
                    label { class: "st-goal-label", r#for: "{field_id}", "{label}" }
                    input {
                        class: "st-goal-input",
                        id: "{field_id}",
                        "data-testid": "stats-daily-{kind}-input",
                        r#type: "number",
                        min: "1",
                        max: "{max}",
                        inputmode: "numeric",
                        value: "{draft()}",
                        disabled: saving(),
                        oninput: move |evt| draft.set(evt.value()),
                    }
                    div { class: "st-goal-actions",
                        button {
                            class: "btn primary",
                            r#type: "button",
                            "data-testid": "stats-daily-{kind}-save",
                            disabled: saving() || offline,
                            onclick: move |_| {
                                match parse_target(&draft(), &save_unit, max) {
                                    Ok(target) => submit_daily(
                                        save_url.clone(),
                                        DailyGoalUpdate::set(&save_kind, target),
                                        daily,
                                        editing,
                                        saving,
                                        error,
                                    ),
                                    Err(msg) => error.set(Some(msg)),
                                }
                            },
                            "Save"
                        }
                        if goal.is_some() {
                            button {
                                class: "btn",
                                r#type: "button",
                                "data-testid": "stats-daily-{kind}-clear",
                                disabled: saving() || offline,
                                onclick: move |_| submit_daily(
                                    clear_url.clone(),
                                    DailyGoalUpdate::clear(&clear_kind),
                                    daily,
                                    editing,
                                    saving,
                                    error,
                                ),
                                "Clear"
                            }
                        }
                        button {
                            class: "btn",
                            r#type: "button",
                            "data-testid": "stats-daily-{kind}-cancel",
                            disabled: saving(),
                            onclick: move |_| {
                                editing.set(false);
                                error.set(None);
                            },
                            "Cancel"
                        }
                    }
                    if offline {
                        p { class: "st-goal-offline", "data-testid": "stats-daily-{kind}-offline",
                            "You're offline — a goal is account settings, so it can't be queued."
                        }
                    }
                }
            }

            if let Some(msg) = error() {
                p {
                    class: "st-goal-error",
                    role: "alert",
                    "data-testid": "stats-daily-{kind}-error",
                    {msg}
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn daily(current: i64, target: i64) -> DailyGoal {
        DailyGoal {
            kind: GOAL_KIND_PAGES.to_string(),
            target,
            current,
            day: "2026-08-29".to_string(),
        }
    }

    #[test]
    fn progress_caption_states_the_real_ratio_and_singularizes_a_one_unit_goal() {
        assert_eq!(progress_caption(3, 24, "book"), "3 of 24 books");
        assert_eq!(progress_caption(0, 1, "book"), "0 of 1 book");
        // Past the target reads as past the target, never clamped to the goal.
        assert_eq!(progress_caption(30, 24, "book"), "30 of 24 books");
        // And the same rules hold for the daily units.
        assert_eq!(progress_caption(12, 30, "page"), "12 of 30 pages");
        assert_eq!(progress_caption(0, 1, "minute"), "0 of 1 minute");
    }

    #[test]
    fn remainder_caption_counts_down_then_reports_the_goal_met() {
        assert_eq!(remainder_caption(23, 24, "book"), "1 book to go");
        assert_eq!(remainder_caption(20, 24, "book"), "4 books to go");
        assert_eq!(remainder_caption(24, 24, "book"), "Goal met");
        assert_eq!(remainder_caption(30, 24, "book"), "Goal met");
        assert_eq!(remainder_caption(29, 30, "page"), "1 page to go");
        assert_eq!(remainder_caption(5, 20, "minute"), "15 minutes to go");
    }

    #[test]
    fn aria_value_now_clamps_into_the_range_aria_requires() {
        // Under and at the target it is just the count.
        assert_eq!(aria_value_now(3, 24), 3);
        assert_eq!(aria_value_now(24, 24), 24);
        // Past it, ARIA forbids exceeding valuemax; `aria-valuetext` carries
        // the honest "30 of 24 books" instead.
        assert_eq!(aria_value_now(30, 24), 24);
        assert_eq!(progress_caption(30, 24, "book"), "30 of 24 books");
    }

    #[test]
    fn parse_target_accepts_in_range_values_and_rejects_the_rest() {
        assert_eq!(parse_target(" 24 ", "book", MAX_GOAL_TARGET), Ok(24));
        assert!(parse_target("", "book", MAX_GOAL_TARGET).is_err());
        assert!(parse_target("twelve", "book", MAX_GOAL_TARGET).is_err());
        assert!(parse_target("0", "book", MAX_GOAL_TARGET).is_err());
        assert!(parse_target(&(MAX_GOAL_TARGET + 1).to_string(), "book", MAX_GOAL_TARGET).is_err());
    }

    #[test]
    fn parse_target_bounds_each_daily_kind_against_its_own_maximum() {
        // 1,500 is a legal day of pages and an impossible day of minutes, so
        // the same draft has to be accepted by one field and refused by the
        // other.
        assert_eq!(parse_target("1500", "page", MAX_DAILY_PAGES), Ok(1_500));
        let err = parse_target("1500", "minute", MAX_DAILY_MINUTES).unwrap_err();
        assert!(err.contains(&MAX_DAILY_MINUTES.to_string()), "{err}");
    }

    #[test]
    fn parse_target_names_the_unit_it_is_asking_for() {
        assert_eq!(
            parse_target("", "page", MAX_DAILY_PAGES),
            Err("Enter a number of pages.".to_string())
        );
        assert_eq!(
            parse_target("x", "minute", MAX_DAILY_MINUTES),
            Err("Enter a whole number of minutes.".to_string())
        );
    }

    #[test]
    fn daily_goal_percent_clamps_but_the_caption_stays_honest() {
        let past = daily(45, 30);
        assert_eq!(past.percent(), 100);
        assert!(past.is_met());
        assert_eq!(past.remaining(), 0);
        assert_eq!(
            progress_caption(past.current, past.target, "page"),
            "45 of 30 pages"
        );
    }

    #[test]
    fn disclosure_minutes_truncates_like_the_goal_it_sits_under() {
        // Reported the same way the goal counts, so the disclosure and the
        // figure above it can't appear to contradict each other.
        assert_eq!(disclosure_minutes(59), 0);
        assert_eq!(disclosure_minutes(60), 1);
        assert_eq!(disclosure_minutes(3_599), 59);
    }
}
