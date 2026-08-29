//! Per-user API-token management (web Settings → API Tokens section), in the
//! `.tkx-*` redesign idiom: blast radius stated up front (the account's
//! permission chips), creation at the top, the one-time secret as a real
//! hand-off (copy + a pre-filled `claude mcp add` command), and a token table
//! with status pills, in-row rename, and in-row revoke confirmation.
//! Signals start empty so SSR and the first WASM paint agree (rule 07).

use dioxus::prelude::*;
use omnibus_shared::ApiTokenView;

use crate::components::user_avatar::initials_for;
use crate::contexts::use_current_user_summary;
use crate::data;
use crate::time::now_unix;

/// Idle threshold: a token unused for longer than this shows the amber
/// "Idle Nd" pill instead of "Active".
const IDLE_AFTER_SECS: i64 = 30 * 86_400;

/// Hint under the create row — the name is the only handle left for
/// deciding what to revoke later.
const CREATE_HINT: &str = "Name it after the machine or client that will hold it — that name is \
                           all you get when deciding what to revoke later.";

/// The instance origin (`https://host`) for absolute `/mcp` URLs. Empty on
/// SSR and the first WASM paint so hydration parity holds (rule 07); a
/// post-mount effect fills it from `window.location.origin` on web, and the
/// rendered URLs re-render from relative to absolute.
pub(super) fn use_instance_origin() -> Signal<String> {
    let origin = use_signal(String::new);
    #[cfg(feature = "web")]
    {
        let mut origin = origin;
        use_effect(move || {
            if let Some(o) = web_sys::window().and_then(|w| w.location().origin().ok()) {
                origin.set(o);
            }
        });
    }
    origin
}

/// Copy `text` to the clipboard. Post-mount web interop only — an SSR no-op,
/// so the markup that triggers it stays identical across targets (rule 07).
#[cfg_attr(not(feature = "web"), allow(unused_variables))]
pub(super) fn copy_to_clipboard(text: &str) {
    #[cfg(feature = "web")]
    {
        let literal = serde_json::to_string(text).unwrap_or_else(|_| String::from("\"\""));
        let _ = dioxus::document::eval(&format!(
            "try {{ navigator.clipboard.writeText({literal}); }} catch (_e) {{}}"
        ));
    }
}

/// The `claude mcp add` one-liner, pre-filled with the instance origin and
/// a bearer value (the real secret, or a `<token>` placeholder).
fn mcp_command(origin: &str, bearer: &str) -> String {
    format!(
        "claude mcp add --transport http omnibus {origin}/mcp --header \
         \"Authorization: Bearer {bearer}\""
    )
}

/// Section body: head + scope strip, create/secret surface, token table (or
/// the numbered-steps empty state). The freshly-minted secret renders in a
/// one-time hand-off panel that survives until dismissed, the page unmounts,
/// or another token is created — it can never be fetched again.
#[component]
pub fn ApiTokensSection() -> Element {
    let mut tokens = use_signal(Vec::<ApiTokenView>::new);
    let mut loaded = use_signal(|| false);
    let mut name_input = use_signal(String::new);
    // (token name, raw secret) of the just-minted token — the only place the
    // secret ever exists client-side.
    let mut new_secret = use_signal(|| None::<(String, String)>);
    let mut confirm_revoke = use_signal(|| None::<i64>);
    let mut rename_target = use_signal(|| None::<i64>);
    let mut rename_input = use_signal(String::new);
    let mut msg = use_signal(|| None::<String>);
    let mut msg_is_error = use_signal(|| false);
    let mut in_flight = use_signal(|| false);
    let origin = use_instance_origin();
    let me = use_current_user_summary();

    use_effect(move || {
        spawn(async move {
            match data::list_api_tokens().await {
                Ok(list) => {
                    tokens.set(list);
                    loaded.set(true);
                }
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
        if in_flight() {
            return;
        }
        in_flight.set(true);
        spawn(async move {
            match data::create_api_token(name).await {
                Ok(created) => {
                    name_input.set(String::new());
                    new_secret.set(Some((created.token.name.clone(), created.secret)));
                    tokens.write().insert(0, created.token);
                    msg.set(None);
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
                    confirm_revoke.set(None);
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

    let on_rename = use_callback(move |id: i64| {
        let name = rename_input().trim().to_string();
        if name.is_empty() {
            msg.set(Some("Enter a name for the token.".to_string()));
            msg_is_error.set(true);
            return;
        }
        if in_flight() {
            return;
        }
        in_flight.set(true);
        spawn(async move {
            match data::rename_api_token(id, name.clone()).await {
                Ok(()) => {
                    if let Some(t) = tokens.write().iter_mut().find(|t| t.id == id) {
                        t.name = name;
                    }
                    rename_target.set(None);
                    msg.set(None);
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
    let count = token_list.len();
    let count_word = if count == 1 { "token" } else { "tokens" };
    let secret = new_secret();
    // The empty state only renders once the list has actually loaded — until
    // then (SSR included) the section shows just head + scope, which both
    // targets paint identically.
    let show_empty = loaded() && count == 0 && secret.is_none();
    let show_create = !show_empty && secret.is_none();
    let (display_name, chips, avatar_initials) = match me() {
        Some(u) => {
            let name = u.display_name.clone().unwrap_or_else(|| u.username.clone());
            let mut chips = Vec::new();
            if u.is_admin {
                chips.push("Admin");
            }
            if u.is_admin || u.can_upload {
                chips.push("Upload");
            }
            if u.is_admin || u.can_edit {
                chips.push("Edit");
            }
            if u.is_admin || u.can_download {
                chips.push("Download");
            }
            let initials = initials_for(&name);
            (name, chips, initials)
        }
        None => (String::new(), Vec::new(), String::new()),
    };

    rsx! {
        div { class: "tkx-tokens", "data-testid": "api-tokens-card",
            div { class: "tkx-head",
                div {
                    h2 { class: "tkx-title", "API Tokens" }
                    p { class: "tkx-lede",
                        "Long-lived tokens let an API client — an MCP server, a script, a "
                        "shortcut — act as you. They never expire on their own; revoking one "
                        "here cuts the client off immediately."
                    }
                }
                span { class: "tkx-count", "data-testid": "api-tokens-count",
                    "{count} live {count_word}"
                }
            }

            div { class: "tkx-scope", "data-testid": "api-tokens-scope",
                span { class: "tkx-scope-who",
                    span { class: "tkx-scope-avatar", title: "{display_name}", "{avatar_initials}" }
                    "Every token carries your account’s permissions, in full:"
                }
                span { class: "tkx-scope-chips",
                    for chip in chips {
                        span { class: "tkx-scope-chip", "{chip}" }
                    }
                }
            }

            if let Some((created_name, raw_secret)) = secret {
                SecretHandOff {
                    name: created_name,
                    secret: raw_secret,
                    origin: origin(),
                    on_dismiss: move |_| new_secret.set(None),
                }
            }

            if show_create {
                section { class: "card",
                    form { class: "tkx-create", onsubmit: on_create,
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
                        button {
                            r#type: "submit",
                            class: "btn primary",
                            disabled: in_flight(),
                            "data-testid": "api-token-create",
                            "Create token"
                        }
                    }
                    p { class: "tkx-create-hint", "{CREATE_HINT}" }
                }
            }

            if show_empty {
                section { class: "card tkx-empty", "data-testid": "api-tokens-empty",
                    div {
                        h3 { class: "tkx-empty-title", "No tokens yet" }
                        p { class: "tkx-lede", "You only need one if something outside this \
                                                browser has to reach your library." }
                    }
                    ol { class: "tkx-steps",
                        li { span { "Name the token after the client that will hold it." } }
                        li { span { "Copy the secret — it is shown once, at creation, and never again." } }
                        li {
                            span {
                                "Paste it into the client, e.g. "
                                code { {mcp_command(&origin(), "<token>")} }
                            }
                        }
                    }
                    form { class: "tkx-create", onsubmit: on_create,
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
                        button {
                            r#type: "submit",
                            class: "btn primary",
                            disabled: in_flight(),
                            "data-testid": "api-token-create",
                            "Create first token"
                        }
                    }
                }
            }

            if !token_list.is_empty() {
                section { class: "card",
                    div { class: "tkx-list-head",
                        span { "Token" }
                        span { "Status" }
                        span { "Created" }
                        span { "Last used" }
                        span {}
                    }
                    ul { class: "tkx-list", "data-testid": "api-token-list",
                        for t in token_list.iter().cloned() {
                            if confirm_revoke() == Some(t.id) {
                                RevokeConfirmRow {
                                    key: "{t.id}",
                                    token: t,
                                    disabled: in_flight(),
                                    on_cancel: move |_| confirm_revoke.set(None),
                                    on_revoke,
                                }
                            } else if rename_target() == Some(t.id) {
                                RenameRow {
                                    key: "{t.id}",
                                    token: t,
                                    disabled: in_flight(),
                                    rename_input,
                                    on_cancel: move |_| rename_target.set(None),
                                    on_rename,
                                }
                            } else {
                                ApiTokenRow {
                                    key: "{t.id}",
                                    token: t.clone(),
                                    disabled: in_flight(),
                                    on_ask_rename: move |id| {
                                        confirm_revoke.set(None);
                                        if let Some(t) = tokens().iter().find(|t| t.id == id) {
                                            rename_input.set(t.name.clone());
                                        }
                                        rename_target.set(Some(id));
                                    },
                                    on_ask_revoke: move |id| {
                                        rename_target.set(None);
                                        confirm_revoke.set(Some(id));
                                    },
                                }
                            }
                        }
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

/// The one-time secret hand-off panel: secret copyfield, the pre-filled
/// `claude mcp add` command, and the explicit dismiss.
#[component]
fn SecretHandOff(
    name: String,
    secret: String,
    origin: String,
    on_dismiss: EventHandler<()>,
) -> Element {
    let command = mcp_command(&origin, &secret);
    let copy_secret = secret.clone();
    let copy_command = command.clone();
    rsx! {
        div { class: "tkx-secret", "data-testid": "api-token-secret-field",
            div { class: "tkx-secret-head",
                h3 { "{name}" }
                span { class: "tkx-secret-once", "shown once" }
            }
            p { class: "tkx-secret-sub",
                "This is the only time the secret is readable. Store it in the client "
                "now — if you lose it, revoke the token and make another."
            }
            div { class: "tkx-copyrow",
                div { class: "tkx-copyfield", "data-testid": "api-token-secret", "{secret}" }
                button {
                    r#type: "button",
                    class: "btn primary",
                    "data-testid": "api-token-secret-copy",
                    onclick: move |_| copy_to_clipboard(&copy_secret),
                    "Copy secret"
                }
            }
            div { class: "tkx-secret-next",
                span { class: "label", "Connect an MCP client" }
                div { class: "tkx-cmd",
                    code { "data-testid": "api-token-mcp-command", "{command}" }
                    button {
                        r#type: "button",
                        class: "btn ghost sm",
                        "data-testid": "api-token-mcp-command-copy",
                        onclick: move |_| copy_to_clipboard(&copy_command),
                        "Copy"
                    }
                }
            }
            div { class: "tkx-secret-done",
                button {
                    r#type: "button",
                    class: "btn",
                    "data-testid": "api-token-secret-dismiss",
                    onclick: move |_| on_dismiss.call(()),
                    "I’ve stored it"
                }
                span { class: "mono", "dismissing hides the secret for good" }
            }
        }
    }
}

/// One token row: name + `omni_…xxxx` identifier, status pill, created and
/// last-used cells, Rename / Revoke actions.
#[component]
fn ApiTokenRow(
    token: ApiTokenView,
    disabled: bool,
    on_ask_rename: EventHandler<i64>,
    on_ask_revoke: EventHandler<i64>,
) -> Element {
    let id = token.id;
    let now = now_unix();
    let (pill_class, pill_label) = status_pill(now, token.last_used_at);
    let created_rel = rel_date(now, token.created_at);
    let created_exact = fmt_timestamp(token.created_at);
    let (used_rel, used_exact) = match token.last_used_at {
        Some(ts) => (rel_date(now, ts), Some(fmt_timestamp(ts))),
        None => ("never".to_string(), None),
    };
    rsx! {
        li { class: "tkx-row", "data-testid": "api-token-row",
            div { class: "tkx-row-name",
                span { class: "tkx-row-title", "{token.name}" }
                // Legacy rows (pre-suffix) render no identifier — omitted,
                // never faked (AC4).
                if let Some(suffix) = token.suffix.as_ref() {
                    span { class: "tkx-row-id", "omni_…{suffix}" }
                }
            }
            span { class: "tkx-pill {pill_class}", "data-testid": "api-token-pill", "{pill_label}" }
            span { class: "tkx-cell", "{created_rel}"
                span { class: "mono", "{created_exact}" }
            }
            span { class: "tkx-cell", "{used_rel}"
                if let Some(exact) = used_exact {
                    span { class: "mono", "{exact}" }
                }
            }
            span { class: "tkx-row-actions",
                button {
                    r#type: "button",
                    class: "btn ghost sm",
                    disabled,
                    "data-testid": "api-token-rename-{id}",
                    onclick: move |_| on_ask_rename.call(id),
                    "Rename"
                }
                button {
                    r#type: "button",
                    class: "btn ghost sm bad",
                    disabled,
                    "data-testid": "api-token-revoke-{id}",
                    onclick: move |_| on_ask_revoke.call(id),
                    "Revoke"
                }
            }
        }
    }
}

/// In-row revoke confirmation, replacing the row until answered.
#[component]
fn RevokeConfirmRow(
    token: ApiTokenView,
    disabled: bool,
    on_cancel: EventHandler<()>,
    on_revoke: Callback<i64>,
) -> Element {
    let id = token.id;
    rsx! {
        li { class: "tkx-row-confirm", "data-testid": "api-token-revoke-confirm",
            span { class: "tkx-confirm-text",
                "Revoke "
                b { "{token.name}" }
                "? The client using it stops working on its next request. This can’t "
                "be undone — you’d have to issue a new token."
            }
            button {
                r#type: "button",
                class: "btn ghost",
                disabled,
                "data-testid": "api-token-revoke-cancel",
                onclick: move |_| on_cancel.call(()),
                "Cancel"
            }
            button {
                r#type: "button",
                class: "btn bad",
                disabled,
                "data-testid": "api-token-revoke-confirm-{id}",
                onclick: move |_| on_revoke.call(id),
                "Revoke token"
            }
        }
    }
}

/// In-row rename form, replacing the row until saved or cancelled.
#[component]
fn RenameRow(
    token: ApiTokenView,
    disabled: bool,
    rename_input: Signal<String>,
    on_cancel: EventHandler<()>,
    on_rename: Callback<i64>,
) -> Element {
    let id = token.id;
    let mut rename_input = rename_input;
    rsx! {
        li { class: "tkx-row-confirm tkx-row-edit", "data-testid": "api-token-rename-row",
            form {
                class: "tkx-rename-form",
                onsubmit: move |evt: Event<FormData>| {
                    evt.prevent_default();
                    on_rename.call(id);
                },
                input {
                    r#type: "text",
                    "data-testid": "api-token-rename-input",
                    "aria-label": "New token name",
                    autocomplete: "off",
                    maxlength: "100",
                    value: "{rename_input}",
                    oninput: move |e| rename_input.set(e.value()),
                }
                button {
                    r#type: "button",
                    class: "btn ghost",
                    disabled,
                    "data-testid": "api-token-rename-cancel",
                    onclick: move |_| on_cancel.call(()),
                    "Cancel"
                }
                button {
                    r#type: "submit",
                    class: "btn primary",
                    disabled,
                    "data-testid": "api-token-rename-save",
                    "Save name"
                }
            }
        }
    }
}

/// Status pill class + label from last-used age: used within
/// [`IDLE_AFTER_SECS`] → Active, older → Idle Nd, never → Never used.
fn status_pill(now: i64, last_used_at: Option<i64>) -> (&'static str, String) {
    match last_used_at {
        None => ("tkx-pill-unused", "Never used".to_string()),
        Some(ts) => {
            let age = (now - ts).max(0);
            if age <= IDLE_AFTER_SECS {
                ("tkx-pill-active", "Active".to_string())
            } else {
                ("tkx-pill-idle", format!("Idle {}d", age / 86_400))
            }
        }
    }
}

/// Coarse relative day display: "today", "yesterday", "N days ago" inside a
/// month, then "14 Jul" (with the year appended once it differs from the
/// current one). The exact date renders in the cell's sub-line.
fn rel_date(now: i64, ts: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let day_now = now.div_euclid(86_400);
    let day_ts = ts.div_euclid(86_400);
    let delta_days = day_now - day_ts;
    match delta_days {
        i64::MIN..=0 => "today".to_string(),
        1 => "yesterday".to_string(),
        2..=30 => format!("{delta_days} days ago"),
        _ => {
            let (y, m, d) = civil_from_days(day_ts);
            let (y_now, _, _) = civil_from_days(day_now);
            let month = MONTHS[(m as usize).saturating_sub(1).min(11)];
            if y == y_now {
                format!("{d} {month}")
            } else {
                format!("{d} {month} {y}")
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

    #[test]
    fn rel_date_covers_today_yesterday_recent_and_dated_tiers() {
        let now = 1_705_276_800 + 43_200; // 2024-01-15T12:00:00Z
        assert_eq!(rel_date(now, now - 3_600), "today");
        assert_eq!(rel_date(now, now - 86_400), "yesterday");
        assert_eq!(rel_date(now, now - 3 * 86_400), "3 days ago");
        // 60 days back → 2023-11-16, a different year than "now".
        assert_eq!(rel_date(now, now - 60 * 86_400), "16 Nov 2023");
        // Same-year fallthrough: pin "now" late in the year.
        let dec = 1_733_011_200; // 2024-12-01T00:00:00Z
        assert_eq!(rel_date(dec, dec - 60 * 86_400), "2 Oct");
    }

    #[test]
    fn status_pill_maps_age_to_active_idle_and_unused() {
        let now = 1_705_276_800;
        assert_eq!(status_pill(now, None).1, "Never used");
        let (cls, label) = status_pill(now, Some(now - 86_400));
        assert_eq!((cls, label.as_str()), ("tkx-pill-active", "Active"));
        let (cls, label) = status_pill(now, Some(now - 91 * 86_400));
        assert_eq!((cls, label.as_str()), ("tkx-pill-idle", "Idle 91d"));
    }

    #[test]
    fn mcp_command_embeds_origin_and_bearer() {
        let cmd = mcp_command("https://example.test", "omni_abcd");
        assert!(cmd.contains("https://example.test/mcp"));
        assert!(cmd.contains("Bearer omni_abcd"));
    }
}

// SSR render-smoke coverage. These need the `server` feature (`dioxus::ssr`).
#[cfg(all(test, feature = "server"))]
mod render_tests {
    use super::*;
    use crate::test_support::render_in_vdom;

    fn section() -> Element {
        rsx! {
            ApiTokensSection {}
        }
    }

    /// First paint: head, scope strip, and create form render; no secret
    /// surface, no rows, and no empty-state card (the list hasn't loaded) —
    /// a secret can only ever appear after a create in the same mounted
    /// session (AC2).
    #[test]
    fn api_tokens_section_first_paint_has_head_and_form_and_no_secret() {
        let html = render_in_vdom(section);
        assert!(html.contains("data-testid=\"api-tokens-card\""));
        assert!(html.contains("data-testid=\"api-token-name-input\""));
        assert!(html.contains("0 live tokens"));
        assert!(!html.contains("data-testid=\"api-token-secret\""));
        assert!(!html.contains("data-testid=\"api-tokens-empty\""));
        assert!(!html.contains("data-testid=\"api-token-row\""));
    }

    /// A row renders name, suffix identifier, pill, dates, and both actions.
    #[test]
    fn api_token_row_renders_identity_pill_dates_and_actions() {
        fn row() -> Element {
            rsx! {
                ApiTokenRow {
                    token: ApiTokenView {
                        id: 7,
                        name: "MCP on my laptop".to_string(),
                        created_at: 1_705_276_800,
                        last_used_at: None,
                        suffix: Some("4e39".to_string()),
                    },
                    disabled: false,
                    on_ask_rename: |_| {},
                    on_ask_revoke: |_| {},
                }
            }
        }
        let html = render_in_vdom(row);
        assert!(html.contains("MCP on my laptop"));
        assert!(html.contains("omni_…4e39"));
        assert!(html.contains("Never used"));
        assert!(html.contains("2024-01-15"));
        assert!(html.contains("data-testid=\"api-token-rename-7\""));
        assert!(html.contains("data-testid=\"api-token-revoke-7\""));
    }

    /// A legacy row (no recorded suffix) omits the identifier line entirely.
    #[test]
    fn api_token_row_omits_identifier_without_a_suffix() {
        fn row() -> Element {
            rsx! {
                ApiTokenRow {
                    token: ApiTokenView {
                        id: 8,
                        name: "legacy".to_string(),
                        created_at: 1_705_276_800,
                        last_used_at: None,
                        suffix: None,
                    },
                    disabled: false,
                    on_ask_rename: |_| {},
                    on_ask_revoke: |_| {},
                }
            }
        }
        let html = render_in_vdom(row);
        assert!(!html.contains("omni_…"));
    }

    /// The hand-off panel carries the secret, the pre-filled command, and
    /// the dismiss control.
    #[test]
    fn secret_hand_off_renders_secret_command_and_dismiss() {
        fn panel() -> Element {
            rsx! {
                SecretHandOff {
                    name: "scan-cron (server)".to_string(),
                    secret: "omni_test_secret".to_string(),
                    origin: "https://example.test".to_string(),
                    on_dismiss: |_| {},
                }
            }
        }
        let html = render_in_vdom(panel);
        assert!(html.contains("data-testid=\"api-token-secret-field\""));
        assert!(html.contains("omni_test_secret"));
        assert!(html.contains("https://example.test/mcp"));
        assert!(html.contains("data-testid=\"api-token-secret-dismiss\""));
    }
}
