use super::*;

#[test]
fn parse_draft_reads_an_empty_field_as_a_deliberate_clear() {
    // The single form has no per-row Clear button; an empty field is how a
    // reader drops a goal, so it must not read as a validation error.
    assert_eq!(parse_draft("", "book", MAX_GOAL_TARGET), Ok(None));
    assert_eq!(parse_draft("   ", "page", MAX_DAILY_PAGES), Ok(None));
}

#[test]
fn parse_draft_accepts_in_range_values_and_rejects_the_rest() {
    assert_eq!(parse_draft(" 24 ", "book", MAX_GOAL_TARGET), Ok(Some(24)));
    assert!(parse_draft("twelve", "book", MAX_GOAL_TARGET).is_err());
    assert!(parse_draft("0", "book", MAX_GOAL_TARGET).is_err());
    assert!(parse_draft(&(MAX_GOAL_TARGET + 1).to_string(), "book", MAX_GOAL_TARGET).is_err());
}

#[test]
fn parse_draft_bounds_each_kind_against_its_own_maximum() {
    // 1,500 is a legal day of pages and an impossible day of minutes, so the
    // same draft has to be accepted by one field and refused by the other.
    assert_eq!(
        parse_draft("1500", "page", MAX_DAILY_PAGES),
        Ok(Some(1_500))
    );
    let err = parse_draft("1500", "minute", MAX_DAILY_MINUTES).unwrap_err();
    assert!(err.contains(&MAX_DAILY_MINUTES.to_string()), "{err}");
}

#[test]
fn parse_draft_names_the_unit_it_is_asking_for() {
    assert_eq!(
        parse_draft("x", "minute", MAX_DAILY_MINUTES),
        Err("Enter a whole number of minutes.".to_string())
    );
}

#[test]
fn target_summary_states_the_unit_or_that_none_is_set() {
    assert_eq!(target_summary(Some(30), "book"), "30 books");
    assert_eq!(target_summary(Some(1), "book"), "1 book");
    assert_eq!(target_summary(Some(45), "minute"), "45 minutes");
    // Never "0" — an unset goal is an absence, not a target of nothing.
    assert_eq!(target_summary(None, "page"), "Not set");
}

#[test]
fn changed_updates_writes_only_the_kinds_that_actually_moved() {
    // The single Save must not restate a target the reader didn't touch:
    // another device may have changed it since this page loaded.
    let current = [Some(30), Some(30), Some(45)];
    let drafts = [Some(30), Some(50), Some(45)];
    assert_eq!(changed_updates(&drafts, &current), vec![(1, Some(50))]);
}

#[test]
fn changed_updates_is_empty_when_nothing_moved() {
    let current = [Some(30), None, Some(45)];
    assert_eq!(changed_updates(&current, &current), Vec::new());
}

#[test]
fn changed_updates_carries_a_clear_as_a_change() {
    // Blanking a field is a write, not a no-op — the row has to be dropped.
    let current = [Some(30), Some(30), Some(45)];
    let drafts = [Some(30), None, Some(45)];
    assert_eq!(changed_updates(&drafts, &current), vec![(1, None)]);
}

#[test]
fn changed_updates_reports_every_moved_kind_in_field_order() {
    let current = [None, None, None];
    let drafts = [Some(30), Some(30), Some(45)];
    assert_eq!(
        changed_updates(&drafts, &current),
        vec![(0, Some(30)), (1, Some(30)), (2, Some(45))]
    );
}

#[test]
fn save_summary_names_the_kinds_that_failed_rather_than_shrugging() {
    // Three goals are three writes. A reader whose annual target saved but
    // whose pages target didn't needs to know which one to retry.
    assert_eq!(save_summary(&[]), ("Goals saved.".to_string(), false));
    let (one, is_error) = save_summary(&["Pages a day"]);
    assert_eq!(
        one,
        "Saved, except Pages a day \u{2014} try that one again."
    );
    assert!(is_error);
    let (two, _) = save_summary(&["Pages a day", "Minutes a day"]);
    assert_eq!(
        two,
        "Saved, except Pages a day and Minutes a day \u{2014} try those again."
    );
}

#[cfg(feature = "server")]
#[test]
fn goals_card_opens_in_read_mode_behind_a_single_edit_control() {
    let html = crate::test_support::render(rsx! { ReadingGoalsCard {} });

    assert!(html.contains("account-goals-card"), "{html}");
    // One control, not one per goal — the whole point of the move.
    assert_eq!(html.matches("goals-edit").count(), 1, "{html}");
    for testid in ["goal-books-value", "goal-pages-value", "goal-minutes-value"] {
        assert!(html.contains(testid), "missing {testid}: {html}");
    }
    // The form is behind the control, not beside it.
    assert!(!html.contains("goals-save"), "{html}");
}
