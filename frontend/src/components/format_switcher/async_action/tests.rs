//! Tests for [`super::AsyncActionToast`]'s spawn-driven state machine.
//! `apply_outcome`/`clear_after_dismiss` carry the race-guard and
//! success/error logic as plain signal writes, so they're exercised
//! directly rather than by driving a real spawned task to completion.

use super::*;
use crate::test_support::render_in_vdom;

fn harness() -> Element {
    let state = use_async_action_toast(1000);
    let in_flight = state.in_flight;
    let result = state.result;
    let in_flight = in_flight();
    let has_result = result().is_some();
    rsx! {
        div { "in_flight:{in_flight} has_result:{has_result}" }
    }
}

#[test]
fn use_async_action_toast_starts_idle_with_no_result() {
    let html = render_in_vdom(harness);
    assert!(html.contains("in_flight:false"));
    assert!(html.contains("has_result:false"));
}

#[test]
fn apply_outcome_ignores_a_superseded_send() {
    #[component]
    fn AssertSuperseded() -> Element {
        let mut in_flight = Signal::new(true);
        let mut result = Signal::new(None::<(bool, String)>);

        let should_dismiss = apply_outcome(
            1,
            2,
            Some((false, "ok".to_string())),
            &mut in_flight,
            &mut result,
        );

        assert!(!should_dismiss);
        assert!(in_flight(), "a superseded send must not touch in_flight");
        assert!(
            result().is_none(),
            "a superseded send must not touch result"
        );
        rsx! {}
    }
    VirtualDom::new(AssertSuperseded).rebuild_in_place();
}

#[test]
fn apply_outcome_applies_a_success_and_signals_dismiss() {
    #[component]
    fn AssertSuccess() -> Element {
        let mut in_flight = Signal::new(true);
        let mut result = Signal::new(None::<(bool, String)>);

        let should_dismiss = apply_outcome(
            1,
            1,
            Some((false, "Sent to Kindle".to_string())),
            &mut in_flight,
            &mut result,
        );

        assert!(
            should_dismiss,
            "a success must schedule the auto-dismiss sleep"
        );
        assert!(!in_flight());
        assert_eq!(result(), Some((false, "Sent to Kindle".to_string())));
        rsx! {}
    }
    VirtualDom::new(AssertSuccess).rebuild_in_place();
}

#[test]
fn apply_outcome_applies_an_error_without_scheduling_dismiss() {
    #[component]
    fn AssertError() -> Element {
        let mut in_flight = Signal::new(true);
        let mut result = Signal::new(None::<(bool, String)>);

        let should_dismiss = apply_outcome(
            1,
            1,
            Some((true, "Send failed: disk full".to_string())),
            &mut in_flight,
            &mut result,
        );

        assert!(
            !should_dismiss,
            "an error toast must persist, not auto-dismiss"
        );
        assert!(!in_flight());
        assert_eq!(result(), Some((true, "Send failed: disk full".to_string())));
        rsx! {}
    }
    VirtualDom::new(AssertError).rebuild_in_place();
}

#[test]
fn apply_outcome_clears_in_flight_quietly_when_outcome_is_none() {
    #[component]
    fn AssertQuiet() -> Element {
        let mut in_flight = Signal::new(true);
        let mut result = Signal::new(None::<(bool, String)>);

        let should_dismiss = apply_outcome(1, 1, None, &mut in_flight, &mut result);

        assert!(!should_dismiss);
        assert!(!in_flight());
        assert!(
            result().is_none(),
            "a quiet outcome (e.g. a cancelled picker) must not raise a toast"
        );
        rsx! {}
    }
    VirtualDom::new(AssertQuiet).rebuild_in_place();
}

#[test]
fn clear_after_dismiss_clears_when_still_latest() {
    #[component]
    fn AssertClears() -> Element {
        let mut result = Signal::new(Some((false, "Sent to Kindle".to_string())));

        clear_after_dismiss(1, 1, &mut result);

        assert!(result().is_none());
        rsx! {}
    }
    VirtualDom::new(AssertClears).rebuild_in_place();
}

#[test]
fn clear_after_dismiss_leaves_a_newer_toast_alone_when_superseded() {
    #[component]
    fn AssertLeavesNewerToast() -> Element {
        let mut result = Signal::new(Some((true, "Send failed: disk full".to_string())));

        clear_after_dismiss(1, 2, &mut result);

        assert_eq!(
            result(),
            Some((true, "Send failed: disk full".to_string())),
            "an old send's dismiss must not clear or hide a newer toast"
        );
        rsx! {}
    }
    VirtualDom::new(AssertLeavesNewerToast).rebuild_in_place();
}
