//! Shared "async action + toast" state machine: the in-flight guard,
//! race-safe auto-dismiss, and toast plumbing common to
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

/// Apply one send's outcome once its action future resolves — the
/// seq-guard-then-write step of [`AsyncActionToast::run`], split out so
/// the race guard and the success/error split are unit-testable without
/// spawning. Returns `true` when this send is still the latest *and* its
/// outcome was a success, i.e. the caller should schedule the auto-dismiss
/// sleep.
fn apply_outcome(
    seq: u64,
    current_seq: u64,
    outcome: Option<(bool, String)>,
    in_flight: &mut Signal<bool>,
    result: &mut Signal<Option<(bool, String)>>,
) -> bool {
    if current_seq != seq {
        return false;
    }
    in_flight.set(false);
    match outcome {
        Some((is_error, message)) => {
            result.set(Some((is_error, message)));
            !is_error
        }
        None => false,
    }
}

/// After the success auto-dismiss sleep, clear the toast only if this send
/// is still the latest — split out so the race guard is unit-testable
/// without spawning or sleeping.
fn clear_after_dismiss(seq: u64, current_seq: u64, result: &mut Signal<Option<(bool, String)>>) {
    if current_seq == seq {
        result.set(None);
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
            let should_dismiss =
                apply_outcome(seq, *seq_sig.peek(), outcome, &mut in_flight, &mut result);
            if should_dismiss {
                async_sleep_ms(success_dismiss_ms).await;
                clear_after_dismiss(seq, *seq_sig.peek(), &mut result);
            }
        });
    }
}

#[cfg(all(test, feature = "server"))]
mod tests;
