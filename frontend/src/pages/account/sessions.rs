//! Self-service "Your sessions" card (web Account section, F5.4 #910).
//!
//! Lists the caller's own live sessions with a per-row Revoke action; the
//! currently-active session (the one authenticating this page) has no
//! Revoke button — the server refuses that revoke anyway (AC2), so the
//! control is hidden rather than offered and rejected. Signals start empty
//! so SSR and the first WASM paint agree (rule 07).

use dioxus::prelude::*;
use omnibus_shared::SessionView;

use crate::data;

/// Card body: session list, loaded on mount and reloaded after every revoke.
#[component]
pub fn SessionsCard() -> Element {
    let mut sessions = use_signal(Vec::<SessionView>::new);
    let mut msg = use_signal(|| None::<String>);
    let mut msg_is_error = use_signal(|| false);
    let mut busy_id = use_signal(|| None::<i64>);
    let mut reload = use_signal(|| 0u32);

    use_effect(move || {
        let _ = reload();
        spawn(async move {
            match data::list_my_sessions().await {
                Ok(list) => sessions.set(list),
                Err(e) => {
                    msg.set(Some(e));
                    msg_is_error.set(true);
                }
            }
        });
    });

    let on_revoke = use_callback(move |id: i64| {
        if busy_id().is_some() {
            return;
        }
        busy_id.set(Some(id));
        spawn(async move {
            match data::revoke_my_session(id).await {
                Ok(()) => {
                    msg.set(Some("Session revoked.".to_string()));
                    msg_is_error.set(false);
                    reload.with_mut(|n| *n += 1);
                }
                Err(e) => {
                    msg.set(Some(e));
                    msg_is_error.set(true);
                }
            }
            busy_id.set(None);
        });
    });

    let session_list = sessions();

    rsx! {
        section { class: "card", "data-testid": "account-sessions-card",
            h2 { "Sessions" }
            p { class: "subtitle", "Devices and browsers currently signed in to your account." }

            {render_session_list(&session_list, busy_id(), on_revoke)}

            if let Some(m) = msg() {
                p {
                    role: "status",
                    "data-testid": "account-sessions-status",
                    class: if msg_is_error() { "settings-status error" } else { "settings-status success" },
                    "{m}"
                }
            }
        }
    }
}

/// The session list, or an empty-state line. Should never actually be empty
/// (the request itself always holds one live session), but the loading
/// window before the effect resolves renders the same empty state as SSR.
fn render_session_list(
    rows: &[SessionView],
    busy_id: Option<i64>,
    on_revoke: Callback<i64>,
) -> Element {
    if rows.is_empty() {
        return rsx! {
            p { class: "settings-status", "data-testid": "account-sessions-empty", "No sessions found." }
        };
    }
    rsx! {
        ul { class: "kobo-device-list", "data-testid": "account-sessions-list",
            for s in rows.iter().cloned() {
                SessionRow {
                    key: "{s.id}",
                    session: s,
                    disabled: busy_id.is_some(),
                    on_revoke,
                }
            }
        }
    }
}

/// One session row: kind + last-used timestamp, and a Revoke button — hidden
/// on the current session (AC2).
#[component]
fn SessionRow(session: SessionView, disabled: bool, on_revoke: Callback<i64>) -> Element {
    let id = session.id;
    rsx! {
        li { class: "kobo-device-row", "data-testid": "account-session-row",
            div { class: "kobo-device-head",
                span { class: "kobo-device-name", "{session.kind}" }
                if session.is_current {
                    span { class: "users-self-tag", " (this device)" }
                }
            }
            p { class: "subtitle", "Last used {fmt_timestamp(session.last_used_at)}" }
            if !session.is_current {
                div { class: "settings-actions",
                    button {
                        r#type: "button",
                        class: "btn ghost danger",
                        disabled,
                        "data-testid": "account-session-revoke-{id}",
                        onclick: move |_| on_revoke.call(id),
                        "Revoke"
                    }
                }
            }
        }
    }
}

/// Render a Unix-seconds timestamp as an ISO-ish date, UTC. Kept tiny and
/// dependency-free (no `chrono` in this leaf component); precision to the
/// day is enough for "which device is this" at a glance.
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
