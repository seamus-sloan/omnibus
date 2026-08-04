//! Shared "credential card" status primitives: the connected/configured dot
//! line and the save/clear/test result message. Reused by the Hardcover and
//! Google Books secret-key cards, the SMTP relay card, and the account
//! Kindle-email card, which otherwise each hand-roll the same two blocks
//! under slightly different wording. Plain functions (not `#[component]`s) —
//! neither needs hooks, so callers can invoke them mid-render without
//! perturbing their own hook order.

use dioxus::prelude::*;

/// The dot + label line reporting whether a credential is configured, e.g.
/// "Connected \u{00b7} settings \u{00b7} hc_l\u{2026}ve" or "Not configured".
/// `testid` becomes the wrapping `data-testid`; `detail` is the full text
/// shown when `configured` — the caller composes the exact "Connected \u{00b7}
/// source \u{00b7} masked" / "Configured \u{00b7} source" wording, since it
/// varies by card.
pub fn credential_status_line(
    testid: &str,
    configured: bool,
    detail: &str,
    unconfigured: &str,
) -> Element {
    rsx! {
        div { class: "api-key-status mono", "data-testid": "{testid}",
            if configured {
                span { class: "api-key-dot connected" }
                "{detail}"
            } else {
                span { class: "api-key-dot" }
                "{unconfigured}"
            }
        }
    }
}

/// The save/clear/test result line: a `role="status"` `<p>` styled success or
/// error, absent until the first action produces a message.
pub fn credential_status_message(testid: &str, msg: Option<&str>, is_error: bool) -> Element {
    let Some(m) = msg else {
        return rsx! {};
    };
    rsx! {
        p {
            role: "status",
            "data-testid": "{testid}",
            class: if is_error { "settings-status error" } else { "settings-status success" },
            "{m}"
        }
    }
}
