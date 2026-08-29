//! Admin toggle card for the hosted `/mcp` endpoint, rendered in the
//! Settings → API Tokens section (the tokens it authenticates with live on
//! the same screen) in the `.tkx-mcp` redesign idiom: admin tag, a real
//! switch with a state word, per-state note text, and a copyable endpoint
//! URL. Default OFF; flipping it takes effect on the next `/mcp` request
//! without a restart. Renders nothing for non-admins — the `is_admin`
//! signal starts `false` on SSR and the first WASM paint, so the two agree
//! (rule 07); the server-side `AdminUser` gate on `/api/settings/mcp` is
//! the real boundary.

use dioxus::prelude::*;

use crate::data;

use super::api_tokens::{copy_to_clipboard, use_instance_origin};

/// Card body: current state loaded on mount (admins only), one
/// enable/disable switch, and the copyable endpoint URL.
#[component]
pub fn McpToggleCard() -> Element {
    let is_admin = crate::use_is_admin();
    let mut enabled = use_signal(|| None::<bool>);
    let mut msg = use_signal(|| None::<String>);
    let mut msg_is_error = use_signal(|| false);
    let mut busy = use_signal(|| false);
    let origin = use_instance_origin();

    use_effect(move || {
        if !is_admin() {
            return;
        }
        spawn(async move {
            match data::mcp_status().await {
                Ok(on) => enabled.set(Some(on)),
                Err(e) => {
                    msg.set(Some(e));
                    msg_is_error.set(true);
                }
            }
        });
    });

    let on_toggle = move |_| {
        let Some(current) = enabled() else { return };
        if busy() {
            return;
        }
        busy.set(true);
        spawn(async move {
            match data::set_mcp_enabled(!current).await {
                Ok(()) => {
                    enabled.set(Some(!current));
                    msg.set(Some(if current {
                        "MCP endpoint disabled. /mcp now answers 404.".to_string()
                    } else {
                        "MCP endpoint enabled at /mcp.".to_string()
                    }));
                    msg_is_error.set(false);
                }
                Err(e) => {
                    msg.set(Some(e));
                    msg_is_error.set(true);
                }
            }
            busy.set(false);
        });
    };

    // Empty for non-admins (and for SSR/first paint, where `is_admin` is
    // still false) — the section's per-user token surface renders either way.
    if !is_admin() {
        return rsx! {};
    }

    let on = enabled() == Some(true);
    let state_word = match enabled() {
        None => "checking…",
        Some(true) => "enabled",
        Some(false) => "disabled",
    };
    let note = match enabled() {
        None => "Checking the endpoint's current state…",
        Some(true) => {
            "Serving this library's MCP tools over streamable HTTP at /mcp. \
             Clients authenticate with a token above."
        }
        Some(false) => "/mcp answers 404 until this is on. Instance-wide, and off by default.",
    };
    let endpoint = format!("{}/mcp", origin());
    let copy_endpoint = endpoint.clone();
    let track_class = if on { "tkx-track on" } else { "tkx-track" };
    let aria_checked = if on { "true" } else { "false" };

    rsx! {
        section { class: "card tkx-mcp", "data-testid": "mcp-toggle-card",
            div { class: "tkx-mcp-head",
                h3 { "Hosted MCP endpoint" }
                span { class: "tkx-admin-tag", "admin" }
                span { class: "tkx-switch",
                    span { class: "tkx-switch-state", "data-testid": "mcp-toggle-state",
                        "{state_word}"
                    }
                    button {
                        r#type: "button",
                        class: "{track_class}",
                        role: "switch",
                        "aria-checked": "{aria_checked}",
                        "aria-label": "Hosted MCP endpoint",
                        disabled: busy() || enabled().is_none(),
                        "data-testid": "mcp-toggle",
                        onclick: on_toggle,
                        span { class: "tkx-knob" }
                    }
                }
            }
            p { class: "tkx-mcp-note", "{note}" }
            div { class: "tkx-copyrow",
                div { class: "tkx-copyfield", "data-testid": "mcp-endpoint-url", "{endpoint}" }
                button {
                    r#type: "button",
                    class: "btn ghost",
                    "data-testid": "mcp-endpoint-copy",
                    onclick: move |_| copy_to_clipboard(&copy_endpoint),
                    "Copy URL"
                }
            }
            if let Some(m) = msg() {
                p {
                    role: "status",
                    "data-testid": "mcp-toggle-status",
                    class: if msg_is_error() { "settings-status error" } else { "settings-status success" },
                    "{m}"
                }
            }
        }
    }
}

// SSR render-smoke coverage. These need the `server` feature (`dioxus::ssr`).
#[cfg(all(test, feature = "server"))]
mod render_tests {
    use super::*;
    use crate::test_support::render_in_vdom;

    /// Outside a resolved admin context (SSR and every non-admin paint) the
    /// card renders nothing — the toggle is never shown to a non-admin.
    #[test]
    fn mcp_toggle_card_renders_nothing_without_admin_context() {
        let html = render_in_vdom(|| rsx! { McpToggleCard {} });
        assert!(!html.contains("mcp-toggle-card"));
    }
}
