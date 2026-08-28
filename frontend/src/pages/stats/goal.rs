//! The annual reading-goal band at the top of `/stats` — a progress bar for a
//! set goal, an invitation when none is, and the inline editor behind both.
//! Anchored to the calendar year, so it sits outside the period-scoped section
//! (directly under the masthead) and never moves when the switcher does.

use dioxus::prelude::*;
use omnibus_shared::{ReadingGoal, ReadingGoalUpdate, MAX_GOAL_TARGET};

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

/// Progress caption for a set goal — the honest ratio, not the clamped
/// percentage, so a reader past their target sees that they are.
fn progress_caption(goal: &ReadingGoal) -> String {
    let noun = if goal.target == 1 { "book" } else { "books" };
    format!("{} of {} {noun}", goal.current, goal.target)
}

/// Trailing status line: what is left, or that the goal is met.
fn remainder_caption(goal: &ReadingGoal) -> String {
    if goal.is_met() {
        return "Goal met".to_string();
    }
    let left = goal.remaining();
    let noun = if left == 1 { "book" } else { "books" };
    format!("{left} {noun} to go")
}

/// `aria-valuenow` for the progress bar. ARIA requires it inside
/// `[aria-valuemin, aria-valuemax]`, so a reader past their target has to be
/// clamped here — the real, unclamped ratio is announced through
/// `aria-valuetext` instead, so nothing is hidden from a screen reader.
fn aria_value_now(goal: &ReadingGoal) -> i64 {
    goal.current.clamp(0, goal.target)
}

/// Parse and bound the editor's draft, returning the message the field should
/// show on rejection. Mirrors `ReadingGoalUpdate::validate`'s bounds so a
/// typo never costs a round trip.
fn parse_target(draft: &str) -> Result<i64, String> {
    let trimmed = draft.trim();
    if trimmed.is_empty() {
        return Err("Enter a number of books.".to_string());
    }
    let Ok(target) = trimmed.parse::<i64>() else {
        return Err("Enter a whole number of books.".to_string());
    };
    if !(1..=MAX_GOAL_TARGET).contains(&target) {
        return Err(format!("Pick a target between 1 and {MAX_GOAL_TARGET}."));
    }
    Ok(target)
}

/// Fire one goal write and fold the server's answer back into the band.
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

/// The goal band. `goal` is the page's current-year goal (owned by the page so
/// a save updates it in place rather than waiting on a refetch), and `year`
/// is the server's calendar year, taken from `StatsSummary::as_of_day` so no
/// client clock reaches the markup.
#[component]
pub fn GoalBand(goal: Signal<Option<ReadingGoal>>, year: String, server_url: String) -> Element {
    // All three seeded to the same value on every target (rule 07): the editor
    // only ever opens from a client click.
    let mut editing = use_signal(|| false);
    let mut draft = use_signal(String::new);
    let saving = use_signal(|| false);
    let mut error: Signal<Option<String>> = use_signal(|| None);

    let offline = is_offline();
    let current = goal.read().clone();

    let save_url = server_url.clone();
    let clear_url = server_url;

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
                    {progress_caption(&g)}
                }
                div {
                    class: "st-goal-track",
                    "data-testid": "stats-goal-progress",
                    role: "progressbar",
                    "aria-valuemin": "0",
                    "aria-valuemax": "{g.target}",
                    "aria-valuenow": "{aria_value_now(&g)}",
                    "aria-valuetext": "{progress_caption(&g)}",
                    "aria-label": "Books finished in {g.year}",
                    div {
                        class: if g.is_met() { "st-goal-fill met" } else { "st-goal-fill" },
                        style: "width: {g.percent()}%",
                    }
                }
                p { class: "st-goal-note", "data-testid": "stats-goal-note",
                    {remainder_caption(&g)}
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
                                match parse_target(&draft()) {
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
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn goal(current: i64, target: i64) -> ReadingGoal {
        ReadingGoal {
            kind: omnibus_shared::GOAL_KIND_BOOKS.to_string(),
            target,
            current,
            year: 2026,
        }
    }

    #[test]
    fn progress_caption_states_the_real_ratio_and_singularizes_a_one_book_goal() {
        assert_eq!(progress_caption(&goal(3, 24)), "3 of 24 books");
        assert_eq!(progress_caption(&goal(0, 1)), "0 of 1 book");
        // Past the target reads as past the target, never clamped to the goal.
        assert_eq!(progress_caption(&goal(30, 24)), "30 of 24 books");
    }

    #[test]
    fn remainder_caption_counts_down_then_reports_the_goal_met() {
        assert_eq!(remainder_caption(&goal(23, 24)), "1 book to go");
        assert_eq!(remainder_caption(&goal(20, 24)), "4 books to go");
        assert_eq!(remainder_caption(&goal(24, 24)), "Goal met");
        assert_eq!(remainder_caption(&goal(30, 24)), "Goal met");
    }

    #[test]
    fn aria_value_now_clamps_into_the_range_aria_requires() {
        // Under and at the target it is just the count.
        assert_eq!(aria_value_now(&goal(3, 24)), 3);
        assert_eq!(aria_value_now(&goal(24, 24)), 24);
        // Past it, ARIA forbids exceeding valuemax; `aria-valuetext` carries
        // the honest "30 of 24 books" instead.
        assert_eq!(aria_value_now(&goal(30, 24)), 24);
        assert_eq!(progress_caption(&goal(30, 24)), "30 of 24 books");
    }

    #[test]
    fn parse_target_accepts_in_range_values_and_rejects_the_rest() {
        assert_eq!(parse_target(" 24 "), Ok(24));
        assert!(parse_target("").is_err());
        assert!(parse_target("twelve").is_err());
        assert!(parse_target("0").is_err());
        assert!(parse_target(&(MAX_GOAL_TARGET + 1).to_string()).is_err());
    }
}
