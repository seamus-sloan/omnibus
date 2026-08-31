use super::*;

fn daily(current: i64, target: i64) -> DailyGoal {
    DailyGoal {
        kind: omnibus_shared::GOAL_KIND_PAGES.to_string(),
        target,
        current,
        day: "2026-08-29".to_string(),
    }
}

fn annual(current: i64, target: i64) -> ReadingGoal {
    ReadingGoal {
        year: 2026,
        kind: omnibus_shared::GOAL_KIND_BOOKS.to_string(),
        target,
        current,
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

#[test]
fn year_fraction_runs_from_one_day_in_to_the_whole_year() {
    let first = year_fraction("2026-01-01").expect("a real day");
    assert!((first - 1.0 / 365.0).abs() < 1e-9, "{first}");
    assert_eq!(year_fraction("2026-12-31"), Some(1.0));
    // 2024 is a leap year, so its last day is still exactly one.
    assert_eq!(year_fraction("2024-12-31"), Some(1.0));
    // A server too old to send the day leaves the pace note unrendered
    // rather than measuring against a guess.
    assert_eq!(year_fraction(""), None);
}

#[test]
fn pace_note_reports_the_gap_against_an_even_spread_of_the_year() {
    // Half a year in against a 30-book target: 15 is on pace, 18 is ahead,
    // 12 is behind. 2026-07-02 is day 183 of 365.
    assert_eq!(
        pace_note(&annual(15, 30), "2026-07-02").as_deref(),
        Some("on pace")
    );
    assert_eq!(
        pace_note(&annual(18, 30), "2026-07-02").as_deref(),
        Some("3 ahead of pace")
    );
    assert_eq!(
        pace_note(&annual(12, 30), "2026-07-02").as_deref(),
        Some("3 behind pace")
    );
}

#[test]
fn pace_note_is_silent_once_the_goal_is_met_or_the_day_is_unknown() {
    // A met goal has no pace left to keep — the ring already says so, and
    // "12 ahead of pace" beside "Goal met" is the same fact twice.
    assert_eq!(pace_note(&annual(30, 30), "2026-07-02"), None);
    assert_eq!(pace_note(&annual(1, 30), "not-a-day"), None);
}

#[test]
fn row_label_supplies_the_timeframe_only_when_the_card_header_has_not() {
    // With a target, "a day" is what the target means.
    assert_eq!(row_label("Pages", true, true), "Pages a day");
    assert_eq!(row_label("Pages", true, false), "Pages a day");
    // No target and no sibling goal: the card header already says "Today", so
    // the row is a bare noun rather than saying it a second time.
    assert_eq!(row_label("Pages", false, true), "Pages");
    // No target but a sibling *is* set, so the header has moved to "Every
    // day" — this row has to carry its own timeframe.
    assert_eq!(row_label("Minutes", false, false), "Minutes today");
}

// SSR render-smoke coverage. The two invitations are `dioxus_router::Link`s,
// which panic without a parent router, so each state gets a one-route test
// router mounted at `/` rather than a bare `render` — same shape as
// `book_detail/highlights/tests.rs`.
#[cfg(feature = "server")]
mod render_tests {
    use super::*;
    use crate::test_support::render_in_vdom;
    use dioxus_router::{Routable, Router};

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum SetRingRoute {
        #[route("/")]
        SetRingHost {},
    }

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum UnsetRingRoute {
        #[route("/")]
        UnsetRingHost {},
    }

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum NoDailyRoute {
        #[route("/")]
        NoDailyHost {},
    }

    #[derive(Clone, Debug, PartialEq, Routable)]
    enum HalfDailyRoute {
        #[route("/")]
        HalfDailyHost {},
    }

    #[component]
    fn SetRingHost() -> Element {
        rsx! {
            AnnualGoalRing {
                goal: Some(annual(19, 30)),
                finished: Some(19),
                year: "2026".to_string(),
                as_of_day: "2026-07-02".to_string(),
            }
        }
    }

    #[component]
    fn UnsetRingHost() -> Element {
        rsx! {
            AnnualGoalRing {
                goal: None,
                finished: Some(22),
                year: "2026".to_string(),
                as_of_day: "2026-07-02".to_string(),
            }
        }
    }

    #[component]
    fn NoDailyHost() -> Element {
        rsx! {
            DailyGoalsCard {
                daily: DailyGoals {
                    pages: None,
                    minutes: None,
                    unzoned_seconds: 0,
                    pages_today: Some(47),
                    minutes_today: Some(0),
                },
            }
        }
    }

    #[component]
    fn HalfDailyHost() -> Element {
        rsx! {
            DailyGoalsCard {
                daily: DailyGoals {
                    pages: Some(daily(18, 30)),
                    minutes: None,
                    unzoned_seconds: 0,
                    pages_today: Some(18),
                    minutes_today: Some(75),
                },
            }
        }
    }

    /// The stats page reports goals and never edits them: all three are set
    /// together in Settings → Account, so no editor may reappear here.
    #[test]
    fn the_goal_cluster_offers_no_editor_only_a_link_to_the_one_that_exists() {
        let set = render_in_vdom(|| rsx! { Router::<SetRingRoute> {} });
        assert!(set.contains("stats-goal-progress"), "{set}");
        assert!(set.contains("19 of 30 books"), "{set}");
        // With a target the ring's own caption names the year, so the kicker
        // only has to say which span it describes.
        assert!(set.contains("This year"), "{set}");
        assert!(!set.contains("so far"), "{set}");
        for gone in ["stats-goal-edit", "stats-goal-input", "stats-goal-save"] {
            assert!(!set.contains(gone), "editor leaked back in ({gone}): {set}");
        }

        // With no target there is no ring — but the year's real count is
        // still reported. The kicker carries the timeframe so it is said
        // once: "This year" over "22 books" over "2026 so far" was the same
        // fact three times.
        let unset = render_in_vdom(|| rsx! { Router::<UnsetRingRoute> {} });
        assert!(unset.contains("stats-goal-today"), "{unset}");
        assert!(unset.contains("22"), "{unset}");
        assert!(unset.contains("2026 so far"), "{unset}");
        assert!(!unset.contains("This year"), "{unset}");
        assert!(unset.contains("stats-goal-set-link"), "{unset}");
        assert!(!unset.contains("stats-goal-progress"), "{unset}");
    }

    /// With no target the row still shows today's figure — the number a
    /// reader wants before deciding what to aim for — with **no bar**, since a
    /// bar is a claim about a target. The header says "Today" rather than
    /// "Every day", which would promise a recurrence nothing is tracking yet,
    /// and because it does, the rows are bare nouns.
    #[test]
    fn the_daily_card_reports_todays_figures_with_no_target_set() {
        let html = render_in_vdom(|| rsx! { Router::<NoDailyRoute> {} });
        assert!(html.contains("Today"), "{html}");
        assert!(!html.contains("Every day"), "{html}");
        assert!(html.contains("stats-daily-pages-today"), "{html}");
        assert!(html.contains("47"), "{html}");
        // A zero is a real answer here, not an absence — the reader has read
        // no minutes today, and an em-dash would claim we don't know.
        assert!(html.contains("stats-daily-minutes-today"), "{html}");
        assert!(
            !html.contains("st-goal-track"),
            "no bar without a target: {html}"
        );
        // The header says "Today" once, so the rows don't repeat it — and
        // the figure is a bare count, since the label already names it.
        assert!(html.contains(">Pages<"), "{html}");
        assert!(html.contains(">Minutes<"), "{html}");
        assert!(!html.contains("Pages a day"), "{html}");
        assert!(!html.contains("Pages today"), "{html}");
        // One short call to action for the card, not one per row.
        assert_eq!(html.matches("stats-daily-set-link").count(), 1, "{html}");
    }

    /// A half-set card runs the targeted kind as a goal and the untargeted one
    /// as a plain readout, so the rows align and the reader can still see what
    /// they did on the kind they haven't committed to.
    #[test]
    fn the_daily_card_mixes_a_goal_and_a_bare_figure_when_one_kind_is_set() {
        let html = render_in_vdom(|| rsx! { Router::<HalfDailyRoute> {} });
        assert!(html.contains("Every day"), "{html}");
        assert!(html.contains("18 of 30 pages"), "{html}");
        assert!(html.contains("stats-daily-minutes-today"), "{html}");
        assert!(html.contains("75"), "{html}");
        // The header moved to "Every day" for the set row's sake, so the
        // untargeted row carries its own timeframe — and only it does.
        assert!(html.contains(">Minutes today<"), "{html}");
        assert!(html.contains(">Pages a day<"), "{html}");
        // And the card still offers the one place to finish setting up.
        assert!(html.contains("stats-daily-set-link"), "{html}");
    }
}
