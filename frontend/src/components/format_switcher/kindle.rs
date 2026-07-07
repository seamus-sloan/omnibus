//! "Send to Kindle" family: the interactive [`SendToKindleButton`], its
//! per-row action helper, and the worker-poll that maps the enqueued job's
//! terminal state to the in-place toast.

use dioxus::prelude::*;

#[cfg(not(feature = "mobile"))]
use omnibus_shared::KindleSendStatus;

// Only the non-mobile `SendToKindleButton`/`poll_send_result` await the shared
// sleeper; the mobile action is a disabled placeholder, so gate to match.
#[cfg(not(feature = "mobile"))]
use super::async_sleep_ms;

/// "Send to Kindle" CTA (F4.3). Web/SSR renders the interactive
/// [`SendToKindleButton`]; mobile renders a disabled placeholder. The cfg gate
/// lives at the helper definition (rule 07: keep cfg out of rsx bodies), and
/// SSR + first WASM paint emit the same enabled button so hydration holds.
#[cfg(not(feature = "mobile"))]
pub(super) fn send_to_kindle_action(uuid: &str, file_id: Option<i64>) -> Element {
    rsx! {
        SendToKindleButton { uuid: uuid.to_string(), file_id }
    }
}

#[cfg(feature = "mobile")]
pub(super) fn send_to_kindle_action(_uuid: &str, _file_id: Option<i64>) -> Element {
    rsx! {
        button {
            class: "btn",
            disabled: true,
            title: "Send-to-Kindle on mobile coming soon",
            "data-testid": "action-kindle",
            "Send to Kindle"
        }
    }
}

/// Interactive Send-to-Kindle button. On click it enqueues the job (fast, so
/// it never trips the server's 30s request-timeout guard) and then polls the
/// worker for the delivery outcome, showing "Sending…" in-place meanwhile. On a
/// terminal state it raises a bottom-center toast (matching the merge/bookmark
/// toasts): a success toast auto-dismisses after a few seconds, an error toast
/// stays until dismissed so the message stays readable. Disabled while in
/// flight. `class` / `testid` default to the per-format-row styling; the hero
/// CTA overrides them to render a large ghost button with its own testid.
#[cfg(not(feature = "mobile"))]
#[component]
pub fn SendToKindleButton(
    uuid: String,
    file_id: Option<i64>,
    #[props(default = "btn".to_string())] class: String,
    #[props(default = "action-kindle".to_string())] testid: String,
) -> Element {
    let server_url = crate::use_server_url();
    let mut in_flight = use_signal(|| false);
    // (is_error, message) — None until the first send completes / toast dismissed.
    let mut result = use_signal(|| None::<(bool, String)>);

    rsx! {
        button {
            class: "{class}",
            disabled: in_flight(),
            "data-testid": "{testid}",
            onclick: move |_| {
                let url = server_url.clone();
                let uuid = uuid.clone();
                in_flight.set(true);
                result.set(None);
                spawn(async move {
                    // Enqueue; a fast pre-check failure (no Kindle email, SMTP
                    // unconfigured, unknown book) comes back here immediately.
                    let task_id = match crate::data::enqueue_send_to_kindle(&url, &uuid, file_id).await {
                        Ok(id) => id,
                        Err(e) => {
                            result.set(Some((true, format!("Send failed: {e}"))));
                            in_flight.set(false);
                            return;
                        }
                    };
                    let (is_error, message) = poll_send_result(&url, task_id).await;
                    result.set(Some((is_error, message)));
                    in_flight.set(false);
                    // Success is transient — auto-dismiss the toast. Errors stay
                    // until the user dismisses them.
                    if !is_error {
                        async_sleep_ms(4000).await;
                        result.set(None);
                    }
                });
            },
            if in_flight() { "Sending\u{2026}" } else { "Send to Kindle" }
        }
        if let Some((is_error, message)) = result() {
            div { class: "kindle-toast card", role: "status",
                span {
                    "data-testid": "kindle-send-status",
                    class: if is_error { "kindle-toast-msg error" } else { "kindle-toast-msg success" },
                    "{message}"
                }
                button {
                    class: "btn ghost sm",
                    "data-testid": "kindle-toast-dismiss",
                    aria_label: "Dismiss",
                    onclick: move |_| result.set(None),
                    "\u{00d7}"
                }
            }
        }
    }
}

/// Poll the worker until the enqueued send reaches a terminal state, returning
/// the toast's `(is_error, message)` pair — `false` on delivery, `true` with an
/// error message otherwise. When the underlying `kindle_send_status` query
/// itself returns `Ok(None)`, the task id went unknown before we saw a terminal
/// state (evicted past the worker's retention window) — rare under sub-second
/// polling, surfaced as a soft error since we can't confirm delivery.
#[cfg(not(feature = "mobile"))]
async fn poll_send_result(url: &str, task_id: u64) -> (bool, String) {
    const POLL_INTERVAL_MS: u32 = 700;
    loop {
        async_sleep_ms(POLL_INTERVAL_MS).await;
        match crate::data::kindle_send_status(url, task_id).await {
            Ok(Some(KindleSendStatus::Pending)) => continue,
            Ok(Some(KindleSendStatus::Sent)) => return (false, "Sent to your Kindle.".to_string()),
            Ok(Some(KindleSendStatus::Failed { message })) => {
                return (true, format!("Send failed: {message}"))
            }
            Ok(None) => {
                return (
                    true,
                    "Send failed: could not confirm the send completed.".to_string(),
                )
            }
            Err(e) => return (true, format!("Send failed: {e}")),
        }
    }
}
