//! Shared "async action + toast" state machine (#2046): the in-flight
//! guard, race-safe auto-dismiss, and toast plumbing common to
//! `kindle::SendToKindleButton` and `kobo::SendToKoboButton`. Each button
//! still owns its own markup and its own action future — only the signals
//! and the spawn/guard logic that drive them are shared.
#![cfg(not(feature = "mobile"))]

use std::future::Future;

use dioxus::prelude::*;

use crate::platform_sleep::async_sleep_ms;

/// State for one async-action-with-toast button: `in_flight` disables the
/// button and swaps its label; `result` drives
/// [`super::send_result_toast`]. Plain signals, so each call site's button
/// markup binds them directly (`disabled: in_flight()`, `if in_flight() {
/// ... }`). `Copy` so a single value can be captured into an `onclick`
/// closure and called from it.
#[derive(Clone, Copy)]
pub(super) struct AsyncActionToast {
    pub in_flight: Signal<bool>,
    pub result: Signal<Option<(bool, String)>>,
    seq: Signal<u64>,
    success_dismiss_ms: u32,
}

/// Build a fresh, idle [`AsyncActionToast`]. `success_dismiss_ms` is how
/// long a success toast stays up before clearing itself — Kindle uses 4000,
/// Kobo 5000, matching each button's pre-extraction behavior.
pub(super) fn use_async_action_toast(success_dismiss_ms: u32) -> AsyncActionToast {
    AsyncActionToast {
        in_flight: use_signal(|| false),
        result: use_signal(|| None),
        seq: use_signal(|| 0u64),
        success_dismiss_ms,
    }
}

impl AsyncActionToast {
    /// Run one send: bump the sequence, flip in-flight and clear any
    /// standing toast, await `action`, then apply its outcome. `action`
    /// resolves to `None` to end quietly (e.g. a cancelled file picker) or
    /// `Some((is_error, message))` to raise a toast — a success toast
    /// auto-dismisses after `success_dismiss_ms`, an error toast persists
    /// until the user dismisses it. A superseded send — a newer click
    /// landed before this one resolved — leaves all shared state alone, so
    /// its auto-dismiss can never clear (or hide the error of) the newer
    /// toast.
    pub fn run(&self, action: impl Future<Output = Option<(bool, String)>> + 'static) {
        let mut in_flight = self.in_flight;
        let mut result = self.result;
        let mut seq_sig = self.seq;
        let success_dismiss_ms = self.success_dismiss_ms;
        let seq = *seq_sig.peek() + 1;
        seq_sig.set(seq);
        in_flight.set(true);
        result.set(None);
        spawn(async move {
            let outcome = action.await;
            if *seq_sig.peek() != seq {
                return;
            }
            in_flight.set(false);
            if let Some((is_error, message)) = outcome {
                result.set(Some((is_error, message)));
                if !is_error {
                    async_sleep_ms(success_dismiss_ms).await;
                    if *seq_sig.peek() == seq {
                        result.set(None);
                    }
                }
            }
        });
    }
}

#[cfg(all(test, feature = "server"))]
mod tests {
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
}
