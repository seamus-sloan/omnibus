//! "Send to Kindle" family: the interactive [`SendToKindleButton`], its
//! per-row action helper, and the worker-poll that maps the enqueued job's
//! terminal state to the in-place toast. The in-flight/toast state machine
//! itself is [`super::async_action::AsyncActionToast`], shared with
//! [`super::kobo::SendToKoboButton`].

use dioxus::prelude::*;
#[cfg(not(feature = "mobile"))]
use omnibus_shared::KindleSendStatus;

#[cfg(not(feature = "mobile"))]
use super::async_action::use_async_action_toast;
// Only `poll_send_result`'s worker-poll interval awaits the shared sleeper
// directly now — the toast auto-dismiss sleep moved into `async_action`.
#[cfg(not(feature = "mobile"))]
use crate::platform_sleep::async_sleep_ms;

/// "Send to Kindle" CTA. Web/SSR renders the interactive
/// [`SendToKindleButton`]; when the EPUB is over Kindle's email cap
/// (`size_bytes` provided by the server, so SSR + first WASM paint agree) the
/// email path can't work, so it becomes a link to Amazon's Send to Kindle page
/// (up to 200 MB) instead — a real action, not a dead disabled button. Mobile
/// renders a disabled placeholder. The cfg gate lives at the helper definition
/// (rule 07: keep cfg out of rsx bodies).
#[cfg(not(feature = "mobile"))]
pub(super) fn send_to_kindle_action(uuid: &str, file_id: Option<i64>, size_bytes: i64) -> Element {
    // `u64::try_from` over an `as` cast so a negative size can't wrap into a
    // huge value; a bad size just falls through to the normal email button (the
    // backend still enforces the real cap).
    if u64::try_from(size_bytes).is_ok_and(omnibus_shared::kindle_email_oversize) {
        return rsx! {
            a {
                class: "btn",
                href: omnibus_shared::KINDLE_WEB_UPLOAD_URL,
                target: "_blank",
                rel: "noopener noreferrer",
                title: "This EPUB is larger than the 50 MB limit for emailing files to a Kindle. Amazon's Send to Kindle page (opens in a new tab) accepts files up to 200 MB.",
                "data-testid": "action-kindle-oversize",
                "Send to Kindle\u{2026}"
            }
        };
    }
    rsx! {
        SendToKindleButton { uuid: uuid.to_string(), file_id }
    }
}

#[cfg(feature = "mobile")]
pub(super) fn send_to_kindle_action(
    _uuid: &str,
    _file_id: Option<i64>,
    _size_bytes: i64,
) -> Element {
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
    // Success toast auto-dismisses after 4s (Kobo's is 5s — each button kept
    // its own pre-extraction timing).
    let action_state = use_async_action_toast(4000);
    let in_flight = action_state.in_flight;
    let result = action_state.result;

    rsx! {
        button {
            class: "{class}",
            disabled: in_flight(),
            "data-testid": "{testid}",
            onclick: move |_| {
                let url = server_url.clone();
                let uuid = uuid.clone();
                action_state.run(async move {
                    // Enqueue; a fast pre-check failure (no Kindle email, SMTP
                    // unconfigured, unknown book) comes back here immediately.
                    let task_id = match crate::data::enqueue_send_to_kindle(&url, &uuid, file_id).await {
                        Ok(id) => id,
                        Err(e) => return Some((true, format!("Send failed: {e}"))),
                    };
                    Some(poll_send_result(&url, task_id).await)
                });
            },
            if in_flight() { "Sending\u{2026}" } else { "Send to Kindle" }
        }
        {super::send_result_toast("kindle", result)}
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

// Every test here renders SSR markup, so the whole module is `server`-gated —
// under `web` its contents would be dead code and CI lints with `-D warnings`.
#[cfg(all(test, feature = "server", not(feature = "mobile")))]
mod tests {
    use super::*;
    use crate::test_support::render_in_vdom;

    fn harness() -> Element {
        rsx! {
            SendToKindleButton {
                uuid: "book-uuid".to_string(),
                file_id: None,
            }
        }
    }

    #[test]
    fn send_to_kindle_button_renders_idle_with_no_toast() {
        let html = render_in_vdom(harness);
        assert!(html.contains("data-testid=\"action-kindle\""));
        assert!(html.contains("Send to Kindle"));
        // The in-flight/result toast only appears after a click resolves —
        // absent on the idle first render.
        assert!(!html.contains("kindle-toast"));
    }

    #[test]
    fn send_to_kindle_button_applies_custom_class_and_testid() {
        let html = render_in_vdom(|| {
            rsx! {
                SendToKindleButton {
                    uuid: "book-uuid".to_string(),
                    file_id: Some(7),
                    class: "btn ghost lg".to_string(),
                    testid: "hero-kindle".to_string(),
                }
            }
        });
        assert!(html.contains("data-testid=\"hero-kindle\""));
        assert!(html.contains("class=\"btn ghost lg\""));
    }
}

// The mobile arm of `send_to_kindle_action` above is a plain hookless
// function (no `use_signal`/context), so `test_support::render` can
// serialize its `Element` directly with no `VirtualDom` needed — unlike the
// interactive `SendToKindleButton` tests above, this exercises the
// `#[cfg(feature = "mobile")]` half of the split (rule 07 worked example),
// so it needs `mobile` *and* `server` together:
// `cargo test -p omnibus-frontend --features mobile,server` (see
// `crate::test_support` for the pattern writeup).
#[cfg(all(test, feature = "mobile", feature = "server"))]
mod mobile_render_tests {
    use crate::test_support::render;

    #[test]
    fn send_to_kindle_action_renders_a_disabled_mobile_placeholder() {
        let html = render(super::send_to_kindle_action("book-uuid", None, 0));
        assert!(html.contains("data-testid=\"action-kindle\""));
        assert!(html.contains("disabled"));
        assert!(html.contains("Send to Kindle"));
        assert!(html.contains("Send-to-Kindle on mobile coming soon"));
    }

    // The mobile placeholder ignores `size_bytes` entirely (no oversize-link
    // branch on this platform), so a size that would trip the oversize link
    // off-mobile still renders the same plain disabled button.
    #[test]
    fn send_to_kindle_action_ignores_size_bytes_on_mobile() {
        let html = render(super::send_to_kindle_action("book-uuid", Some(3), i64::MAX));
        assert!(html.contains("data-testid=\"action-kindle\""));
        assert!(!html.contains("action-kindle-oversize"));
    }
}
