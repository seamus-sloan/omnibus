//! Admin toggle card for the hosted `/mcp` endpoint, rendered in the
//! Settings → API Tokens section (the tokens it authenticates with live on
//! the same screen). Default OFF; flipping it takes effect on the next
//! `/mcp` request without a restart. Renders nothing for non-admins — the
//! `is_admin` signal starts `false` on SSR and the first WASM paint, so the
//! two agree (rule 07); the server-side `AdminUser` gate on
//! `/api/settings/mcp` is the real boundary.

use dioxus::prelude::*;

use crate::data;

/// Card body: current state loaded on mount (admins only), one
/// enable/disable action.
#[component]
pub fn McpToggleCard() -> Element {
    let is_admin = crate::use_is_admin();
    let mut enabled = use_signal(|| None::<bool>);
    let mut msg = use_signal(|| None::<String>);
    let mut msg_is_error = use_signal(|| false);
    let mut busy = use_signal(|| false);

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
    // still false) — the section's per-user token card renders either way.
    if !is_admin() {
        return rsx! {};
    }

    let state_line = match enabled() {
        None => "Checking…",
        Some(true) => "Enabled — MCP clients can connect to /mcp.",
        Some(false) => "Disabled — /mcp answers 404 (the default).",
    };
    let button_label = match enabled() {
        Some(true) => "Disable MCP endpoint",
        _ => "Enable MCP endpoint",
    };

    rsx! {
        section { class: "card", "data-testid": "mcp-toggle-card",
            h2 { "Hosted MCP endpoint" }
            p { class: "subtitle",
                "Serve this library's MCP tools over streamable HTTP at "
                code { "/mcp" }
                ". Instance-wide and off by default. MCP clients authenticate "
                "with an API token from the card above:"
            }
            p { class: "subtitle",
                code {
                    "claude mcp add --transport http omnibus https://<host>/mcp "
                    "--header \"Authorization: Bearer <api-token>\""
                }
            }
            p { class: "settings-status", "data-testid": "mcp-toggle-state", "{state_line}" }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn",
                    disabled: busy() || enabled().is_none(),
                    "data-testid": "mcp-toggle",
                    onclick: on_toggle,
                    "{button_label}"
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
