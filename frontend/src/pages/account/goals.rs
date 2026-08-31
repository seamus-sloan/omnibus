//! Reading-goals card in Settings → Account: the annual books target and the
//! two standing daily targets, edited together behind one control.
//!
//! One editor rather than three, even though the write path is per-kind: a
//! reader setting up their year thinks about all three at once, and three
//! separate Edit buttons scattered across a stats page made a single decision
//! feel like three chores. [`changed_updates`] is what keeps the per-kind
//! contract intact — only the kinds whose value actually moved are written, so
//! a save never restates a target the reader didn't touch.
//!
//! Goals are account configuration, so every write goes straight to the server
//! and surfaces its failure inline (rule 08 test 1). Signals start empty so SSR
//! and the first WASM paint agree (rule 07).

use dioxus::prelude::*;
use omnibus_shared::{
    DailyGoalUpdate, DailyGoals, ReadingGoal, ReadingGoalUpdate, StatsRange, GOAL_KIND_MINUTES,
    GOAL_KIND_PAGES, MAX_DAILY_MINUTES, MAX_DAILY_PAGES, MAX_GOAL_TARGET,
};

use crate::components::credential_card::credential_status_message;
use crate::{data, use_server_url};

/// One editable goal: which kind it is, what it's called, and what bounds it.
struct GoalField {
    /// `None` for the annual goal, which has its own route and no kind param.
    kind: Option<&'static str>,
    label: &'static str,
    unit: &'static str,
    hint: &'static str,
    max: i64,
    testid: &'static str,
}

/// The three goals, in the order a reader sets them: the year first, then the
/// two dailies that feed it.
const FIELDS: [GoalField; 3] = [
    GoalField {
        kind: None,
        label: "Books this year",
        unit: "book",
        hint: "Counts a book once, the day you finish it.",
        max: MAX_GOAL_TARGET,
        testid: "goal-books",
    },
    GoalField {
        kind: Some(GOAL_KIND_PAGES),
        label: "Pages a day",
        unit: "page",
        hint: "Estimated from how far you move through each book.",
        max: MAX_DAILY_PAGES,
        testid: "goal-pages",
    },
    GoalField {
        kind: Some(GOAL_KIND_MINUTES),
        label: "Minutes a day",
        unit: "minute",
        hint: "Reading and listening together, on your own clock.",
        max: MAX_DAILY_MINUTES,
        testid: "goal-minutes",
    },
];

/// Signals backing the card — grouped so the seeding effect and the save
/// handler don't each take six signal params. Mirrors `ProfileSignals` in
/// `profile.rs`.
#[derive(Clone, Copy)]
struct GoalSignals {
    /// One draft per [`FIELDS`] entry, in the same order. Empty means "no
    /// target", which is how a goal is cleared.
    drafts: Signal<[String; 3]>,
    editing: Signal<bool>,
    saving: Signal<bool>,
    msg: Signal<Option<String>>,
    msg_is_error: Signal<bool>,
    /// The server's current targets, in [`FIELDS`] order.
    targets: Signal<[Option<i64>; 3]>,
    loaded: Signal<bool>,
}

/// Pluralize a unit noun. The hints read as sentences, so "1 book" has to win
/// over a parenthesised "(s)".
fn plural(unit: &str, n: i64) -> String {
    if n == 1 {
        unit.to_string()
    } else {
        format!("{unit}s")
    }
}

/// Parse and bound one draft. An empty draft is a deliberate clear, not an
/// error — it is how the single form expresses "no target" without needing a
/// Clear button per row.
///
/// Mirrors the write path's own bounds so a typo never costs a round trip.
fn parse_draft(draft: &str, unit: &str, max: i64) -> Result<Option<i64>, String> {
    let trimmed = draft.trim();
    if trimmed.is_empty() {
        return Ok(None);
    }
    let Ok(target) = trimmed.parse::<i64>() else {
        return Err(format!("Enter a whole number of {}.", plural(unit, 2)));
    };
    if !(1..=max).contains(&target) {
        return Err(format!("Pick a target between 1 and {max}."));
    }
    Ok(Some(target))
}

/// Read-mode value for one goal: the target with its unit, or that none is set.
fn target_summary(target: Option<i64>, unit: &str) -> String {
    match target {
        Some(n) => format!("{n} {}", plural(unit, n)),
        None => "Not set".to_string(),
    }
}

/// The drafts a save should actually write: index and new value for every
/// field whose target moved.
///
/// This is what preserves the per-kind write contract behind a single Save. A
/// reader changing their pages target has said nothing about their minutes
/// target, and restating it would overwrite a value another device may have
/// changed since this page loaded.
fn changed_updates(
    drafts: &[Option<i64>; 3],
    current: &[Option<i64>; 3],
) -> Vec<(usize, Option<i64>)> {
    (0..FIELDS.len())
        .filter(|&i| drafts[i] != current[i])
        .map(|i| (i, drafts[i]))
        .collect()
}

/// The status line after a save: what landed, or which kinds didn't.
///
/// Partial failure is reported by name rather than collapsed into "something
/// went wrong": three goals are three writes, and a reader whose annual target
/// saved but whose pages target didn't needs to know which one to retry.
fn save_summary(failed: &[&'static str]) -> (String, bool) {
    match failed {
        [] => ("Goals saved.".to_string(), false),
        [one] => (
            format!("Saved, except {one} \u{2014} try that one again."),
            true,
        ),
        many => (
            format!(
                "Saved, except {} \u{2014} try those again.",
                many.join(" and ")
            ),
            true,
        ),
    }
}

/// Seed the drafts and targets from the server's current goals, once.
///
/// Reads them off the all-time summary — the one payload that already carries
/// both `goal` and `daily_goals`, and the same read the stats page makes, so
/// this costs a shared cache hit rather than a new endpoint.
fn use_goal_hydration(server_url: String, mut signals: GoalSignals) {
    use_effect(move || {
        if (signals.loaded)() {
            return;
        }
        let url = server_url.clone();
        spawn(async move {
            let Ok(summary) = data::fetch_stats(&url, StatsRange::AllTime).await else {
                return;
            };
            let targets = [
                summary.goal.as_ref().map(|g: &ReadingGoal| g.target),
                summary.daily_goals.pages.as_ref().map(|g| g.target),
                summary.daily_goals.minutes.as_ref().map(|g| g.target),
            ];
            signals.targets.set(targets);
            signals
                .drafts
                .set(targets.map(|t| t.map(|n| n.to_string()).unwrap_or_default()));
            signals.loaded.set(true);
        });
    });
}

/// Write every changed goal and fold the results back into the card.
///
/// The writes run in sequence rather than concurrently: they are three
/// requests against one user's settings, and a failure part-way through has to
/// leave the card describing what actually landed.
fn save_handler(
    server_url: String,
    mut signals: GoalSignals,
) -> impl FnMut(Event<FormData>) + 'static {
    move |evt: Event<FormData>| {
        evt.prevent_default();
        let drafts = (signals.drafts)();
        let mut parsed = [None; 3];
        for (i, field) in FIELDS.iter().enumerate() {
            match parse_draft(&drafts[i], field.unit, field.max) {
                Ok(value) => parsed[i] = value,
                Err(msg) => {
                    signals.msg.set(Some(format!("{}: {msg}", field.label)));
                    signals.msg_is_error.set(true);
                    return;
                }
            }
        }
        let pending = changed_updates(&parsed, &(signals.targets)());
        if pending.is_empty() {
            signals.editing.set(false);
            signals.msg.set(None);
            return;
        }
        let url = server_url.clone();
        signals.saving.set(true);
        spawn(async move {
            let mut landed = (signals.targets)();
            let mut failed: Vec<&'static str> = Vec::new();
            for (i, value) in pending {
                let ok = match FIELDS[i].kind {
                    None => write_annual(&url, value).await,
                    Some(kind) => write_daily(&url, kind, value).await,
                };
                if ok {
                    landed[i] = value;
                } else {
                    failed.push(FIELDS[i].label);
                }
            }
            let (msg, is_error) = save_summary(&failed);
            signals.targets.set(landed);
            // Reset only the drafts that landed. Resetting all of them would
            // overwrite what the reader typed into a kind whose write failed
            // with the value it had before — and since the next Save diffs
            // drafts against targets, the retry the status line asks for would
            // then find no change and close the editor having written nothing.
            let mut drafts_after = (signals.drafts)();
            for (i, field) in FIELDS.iter().enumerate() {
                if !failed.contains(&field.label) {
                    drafts_after[i] = landed[i].map(|n| n.to_string()).unwrap_or_default();
                }
            }
            signals.drafts.set(drafts_after);
            signals.msg.set(Some(msg));
            signals.msg_is_error.set(is_error);
            signals.editing.set(!failed.is_empty());
            signals.saving.set(false);
        });
    }
}

/// One annual write. `None` clears the target, keeping the year's progress.
async fn write_annual(server_url: &str, target: Option<i64>) -> bool {
    let update = match target {
        Some(n) => ReadingGoalUpdate::books(n),
        None => ReadingGoalUpdate::clear_books(),
    };
    data::save_reading_goal(server_url, &update).await.is_ok()
}

/// One daily write. `None` clears the target only — the day's progress is the
/// server's to report and survives, as `DailyGoalUpdate::clear` does.
async fn write_daily(server_url: &str, kind: &str, target: Option<i64>) -> bool {
    let update = match target {
        Some(n) => DailyGoalUpdate::set(kind, n),
        None => DailyGoalUpdate::clear(kind),
    };
    data::save_daily_goal(server_url, &update)
        .await
        .map(|_: DailyGoals| ())
        .is_ok()
}

/// The three targets as a read-only list, over the one control that opens them
/// all for editing.
fn goals_readout(
    targets: [Option<i64>; 3],
    on_edit: impl FnMut(Event<MouseData>) + 'static,
) -> Element {
    rsx! {
        dl { class: "goal-readout",
            for (i, field) in FIELDS.iter().enumerate() {
                div { key: "{field.testid}", class: "goal-readout-row",
                    dt { class: "goal-readout-label", {field.label} }
                    dd {
                        class: if targets[i].is_some() { "goal-readout-value" } else { "goal-readout-value unset" },
                        "data-testid": "{field.testid}-value",
                        {target_summary(targets[i], field.unit)}
                    }
                }
            }
        }
        div { class: "settings-actions",
            button {
                r#type: "button",
                class: "btn",
                "data-testid": "goals-edit",
                onclick: on_edit,
                "Edit goals"
            }
        }
    }
}

/// The three fields as one form. A blank field clears that goal, which is why
/// there is no Clear button per row.
fn goals_form(
    mut drafts: Signal<[String; 3]>,
    mut msg: Signal<Option<String>>,
    saving: bool,
    on_save: impl FnMut(Event<FormData>) + 'static,
    on_cancel: impl FnMut(Event<MouseData>) + 'static,
) -> Element {
    let current = drafts();
    rsx! {
        // `novalidate`: the browser's own constraint check would silently
        // refuse the submit on an over-`max` value, so the card's error — which
        // names the field and its bound — would never run. `min`/`max` stay for
        // the spinner and the mobile keypad.
        form {
            id: "goals-form",
            class: "settings-form",
            novalidate: true,
            onsubmit: on_save,
            for (i, field) in FIELDS.iter().enumerate() {
                div { key: "{field.testid}", class: "settings-field",
                    label { r#for: "{field.testid}", {field.label} }
                    input {
                        r#type: "number",
                        id: "{field.testid}",
                        "data-testid": "{field.testid}-input",
                        min: "1",
                        max: "{field.max}",
                        inputmode: "numeric",
                        placeholder: "No target",
                        value: "{current[i]}",
                        disabled: saving,
                        oninput: move |e| {
                            let mut next = drafts();
                            next[i] = e.value();
                            drafts.set(next);
                            msg.set(None);
                        },
                    }
                    p { class: "subtitle", {field.hint} }
                }
            }
            p { class: "subtitle",
                "Leave a field empty to drop that goal. Clearing a target keeps the progress \
                 you have already made toward it."
            }
            div { class: "settings-actions",
                button {
                    r#type: "submit",
                    class: "btn",
                    disabled: saving,
                    "data-testid": "goals-save",
                    "Save"
                }
                button {
                    r#type: "button",
                    class: "btn ghost",
                    disabled: saving,
                    "data-testid": "goals-cancel",
                    onclick: on_cancel,
                    "Cancel"
                }
            }
        }
    }
}

/// Reading goals for the signed-in user — the one place all three are set.
///
/// The stats page renders them but never edits them: a goal is account
/// configuration, and scattering its editors across the surface that *reports*
/// it is what made a single decision feel like three.
#[component]
pub(crate) fn ReadingGoalsCard() -> Element {
    let server_url = use_server_url();
    let mut signals = GoalSignals {
        drafts: use_signal(|| [const { String::new() }; 3]),
        editing: use_signal(|| false),
        saving: use_signal(|| false),
        msg: use_signal(|| None::<String>),
        msg_is_error: use_signal(|| false),
        targets: use_signal(|| [None; 3]),
        loaded: use_signal(|| false),
    };
    use_goal_hydration(server_url.clone(), signals);

    let on_save = save_handler(server_url, signals);
    let on_edit = move |_| {
        signals.msg.set(None);
        signals.editing.set(true);
    };
    let on_cancel = move |_| {
        // Drop the drafts back to what the server last confirmed, so a
        // cancelled edit can't leave a half-typed target on screen.
        signals
            .drafts
            .set((signals.targets)().map(|t| t.map(|n| n.to_string()).unwrap_or_default()));
        signals.msg.set(None);
        signals.editing.set(false);
    };

    rsx! {
        section { class: "card", "data-testid": "account-goals-card",
            h2 { "Reading goals" }
            p { class: "subtitle",
                "One target for the year and two for the day. They show on your Stats page."
            }
            if (signals.editing)() {
                {goals_form(signals.drafts, signals.msg, (signals.saving)(), on_save, on_cancel)}
            } else {
                {goals_readout((signals.targets)(), on_edit)}
            }
            {credential_status_message(
                "goals-status",
                (signals.msg)().as_deref(),
                (signals.msg_is_error)(),
            )}
        }
    }
}

#[cfg(test)]
mod tests;
