//! Per-user API-token management card (web Settings → API Tokens section).
//!
//! Creates long-lived `omni_…` tokens (the secret is shown exactly once, at
//! creation), lists them with name/created/last-used, and revokes them.
//! Signals start empty so SSR and the first WASM paint agree (rule 07).

use dioxus::prelude::*;
use omnibus_shared::ApiTokenView;

use crate::data;

/// Card body: token list + create form, loaded on mount. The freshly-minted
/// secret renders in a one-time banner that survives until the page unmounts
/// or another token is created — it can never be fetched again.
#[component]
pub fn ApiTokensSection() -> Element {
    let mut tokens = use_signal(Vec::<ApiTokenView>::new);
    let mut name_input = use_signal(String::new);
    let mut new_secret = use_signal(|| None::<String>);
    let mut msg = use_signal(|| None::<String>);
    let mut msg_is_error = use_signal(|| false);
    let mut in_flight = use_signal(|| false);

    use_effect(move || {
        spawn(async move {
            match data::list_api_tokens().await {
                Ok(list) => tokens.set(list),
                Err(e) => {
                    msg.set(Some(e.to_string()));
                    msg_is_error.set(true);
                }
            }
        });
    });

    let on_create = move |evt: Event<FormData>| {
        evt.prevent_default();
        let name = name_input().trim().to_string();
        if name.is_empty() {
            msg.set(Some("Enter a name for the token.".to_string()));
            msg_is_error.set(true);
            return;
        }
        in_flight.set(true);
        spawn(async move {
            match data::create_api_token(name).await {
                Ok(created) => {
                    name_input.set(String::new());
                    new_secret.set(Some(created.secret));
                    tokens.write().insert(0, created.token);
                    msg.set(Some(
                        "Token created. Copy the secret now — it won't be shown again.".to_string(),
                    ));
                    msg_is_error.set(false);
                }
                Err(e) => {
                    msg.set(Some(e.to_string()));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    };

    let on_revoke = use_callback(move |id: i64| {
        if in_flight() {
            return;
        }
        in_flight.set(true);
        spawn(async move {
            match data::revoke_api_token(id).await {
                Ok(()) => {
                    tokens.write().retain(|t| t.id != id);
                    msg.set(Some("Token revoked.".to_string()));
                    msg_is_error.set(false);
                }
                Err(e) => {
                    msg.set(Some(e.to_string()));
                    msg_is_error.set(true);
                }
            }
            in_flight.set(false);
        });
    });

    let token_list = tokens();

    rsx! {
        section { class: "card", "data-testid": "api-tokens-card",
            h2 { "API Tokens" }
            p { class: "subtitle",
                "Long-lived tokens for connecting API clients (like the Omnibus MCP "
                "server) to your account. A token has exactly your account's "
                "permissions and never expires on its own — revoke it here when a "
                "client should lose access. Treat each token like a password."
            }

            if let Some(secret) = new_secret() {
                div { class: "settings-field", "data-testid": "api-token-secret-field",
                    label { r#for: "api-token-secret", "New token — copy it now, it won't be shown again" }
                    input {
                        r#type: "text",
                        id: "api-token-secret",
                        class: "kobo-endpoint-url",
                        "data-testid": "api-token-secret",
                        readonly: true,
                        value: "{secret}",
                    }
                }
            }

            {render_token_list(&token_list, in_flight(), on_revoke)}

            form {
                id: "api-token-create-form",
                class: "settings-form",
                onsubmit: on_create,
                div { class: "settings-field",
                    label { r#for: "api-token-name", "Token name" }
                    input {
                        r#type: "text",
                        id: "api-token-name",
                        name: "api_token_name",
                        "data-testid": "api-token-name-input",
                        autocomplete: "off",
                        placeholder: "MCP on my laptop",
                        maxlength: "100",
                        value: "{name_input}",
                        oninput: move |e| name_input.set(e.value()),
                    }
                }
                div { class: "settings-actions",
                    button {
                        r#type: "submit",
                        class: "btn",
                        disabled: in_flight(),
                        "data-testid": "api-token-create",
                        "Create token"
                    }
                }
            }

            if let Some(m) = msg() {
                p {
                    role: "status",
                    "data-testid": "api-tokens-status",
                    class: if msg_is_error() { "settings-status error" } else { "settings-status success" },
                    "{m}"
                }
            }
        }
    }
}

/// The token list, or the empty-state line — also what the pre-fetch SSR
/// paint renders, so the two match.
fn render_token_list(rows: &[ApiTokenView], disabled: bool, on_revoke: Callback<i64>) -> Element {
    if rows.is_empty() {
        return rsx! {
            p { class: "settings-status", "data-testid": "api-tokens-empty", "No API tokens yet." }
        };
    }
    rsx! {
        ul { class: "kobo-device-list", "data-testid": "api-token-list",
            for t in rows.iter().cloned() {
                ApiTokenRow { key: "{t.id}", token: t, disabled, on_revoke }
            }
        }
    }
}

/// One token row: name, created/last-used dates, and its Revoke action.
#[component]
fn ApiTokenRow(token: ApiTokenView, disabled: bool, on_revoke: Callback<i64>) -> Element {
    let id = token.id;
    let last_used = match token.last_used_at {
        Some(ts) => format!("last used {}", fmt_timestamp(ts)),
        None => "never used".to_string(),
    };
    rsx! {
        li { class: "kobo-device-row", "data-testid": "api-token-row",
            div { class: "kobo-device-head",
                span { class: "kobo-device-name", "{token.name}" }
            }
            p { class: "subtitle", "Created {fmt_timestamp(token.created_at)} · {last_used}" }
            div { class: "settings-actions",
                button {
                    r#type: "button",
                    class: "btn ghost danger",
                    disabled,
                    "data-testid": "api-token-revoke-{id}",
                    onclick: move |_| on_revoke.call(id),
                    "Revoke"
                }
            }
        }
    }
}

/// Render a Unix-seconds timestamp as an ISO-ish date, UTC. Mirrors
/// `pages::account::sessions::fmt_timestamp` (private there; duplicated tiny
/// rather than widening its visibility for a leaf helper).
fn fmt_timestamp(unix_secs: i64) -> String {
    let days = unix_secs.div_euclid(86_400);
    let (y, m, d) = civil_from_days(days);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Days since the Unix epoch → `(year, month, day)` civil date (Howard
/// Hinnant's `civil_from_days`). Mirrors `pages::settings::users::civil_from_days`.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = u32::try_from(doy - (153 * mp + 2) / 5 + 1).unwrap_or(1);
    let m = u32::try_from(if mp < 10 { mp + 3 } else { mp - 9 }).unwrap_or(1);
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fmt_timestamp_renders_iso_date() {
        // 2024-01-15T00:00:00Z
        assert_eq!(fmt_timestamp(1_705_276_800), "2024-01-15");
    }
}

// SSR render-smoke coverage. These need the `server` feature (`dioxus::ssr`).
#[cfg(all(test, feature = "server"))]
mod render_tests {
    use super::*;
    use crate::test_support::render_in_vdom;

    fn card() -> Element {
        rsx! {
            ApiTokensSection {}
        }
    }

    /// First paint: the create form and empty state render, and no secret
    /// surface exists — a secret can only ever appear after a create in the
    /// same mounted session (AC2).
    #[test]
    fn api_tokens_card_first_paint_has_form_and_no_secret() {
        let html = render_in_vdom(card);
        assert!(html.contains("data-testid=\"api-tokens-card\""));
        assert!(html.contains("data-testid=\"api-token-name-input\""));
        assert!(html.contains("data-testid=\"api-tokens-empty\""));
        assert!(!html.contains("data-testid=\"api-token-secret\""));
    }

    /// A row renders name, dates, and its revoke control.
    #[test]
    fn api_token_row_renders_name_dates_and_revoke() {
        fn row() -> Element {
            rsx! {
                ApiTokenRow {
                    token: ApiTokenView {
                        id: 7,
                        name: "MCP on my laptop".to_string(),
                        created_at: 1_705_276_800,
                        last_used_at: None,
                    },
                    disabled: false,
                    on_revoke: Callback::new(|_| {}),
                }
            }
        }
        let html = render_in_vdom(row);
        assert!(html.contains("MCP on my laptop"));
        assert!(html.contains("2024-01-15"));
        assert!(html.contains("never used"));
        assert!(html.contains("data-testid=\"api-token-revoke-7\""));
    }
}
