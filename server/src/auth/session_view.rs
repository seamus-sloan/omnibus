//! Projects `sessions` rows onto the [`SessionView`] wire type, naming the
//! client that holds each one. The single projection for both listings — the
//! self-service `GET /api/auth/sessions` and the admin
//! `GET /api/admin/users/{id}/sessions` — so the two can't drift on how a
//! client is named. Server-side by design: the raw `User-Agent` stays in the
//! database and only the derived label crosses the wire.

use std::collections::HashMap;

use omnibus_db::auth::{AuthResult, Session};
use omnibus_shared::{SessionView, UNKNOWN_CLIENT};
use sqlx::SqlitePool;

/// Project `sessions` onto their wire views, resolving each one's client name.
///
/// `current_session_id` marks the row the request itself authenticated with;
/// the admin listing passes `None` because it never sets that flag (see
/// [`SessionView::is_current`]). Registered devices are fetched once for the
/// whole listing rather than per row.
pub async fn session_views(
    pool: &SqlitePool,
    user_id: i64,
    sessions: Vec<Session>,
    current_session_id: Option<i64>,
) -> AuthResult<Vec<SessionView>> {
    let devices: HashMap<i64, String> = omnibus_db::auth::list_devices_for_user(pool, user_id)
        .await?
        .into_iter()
        .map(|d| (d.id, d.name))
        .collect();

    Ok(sessions
        .into_iter()
        .map(|s| {
            let device_name = s
                .device_id
                .and_then(|id| devices.get(&id))
                .map(String::as_str);
            SessionView {
                id: s.id,
                device_id: s.device_id,
                kind: s.kind.as_str().to_string(),
                client: client_label(device_name, s.user_agent.as_deref()),
                created_at: s.created_at,
                last_used_at: s.last_used_at,
                expires_at: s.expires_at,
                is_current: Some(s.id) == current_session_id,
            }
        })
        .collect())
}

/// Name the client holding a session.
///
/// A registered device wins outright: the native clients send their own name
/// (`UIDevice.current.name`), which beats anything guessable from a header.
/// Web logins register no device, so those fall through to the `User-Agent`.
pub fn client_label(device_name: Option<&str>, user_agent: Option<&str>) -> String {
    if let Some(name) = device_name.map(str::trim).filter(|n| !n.is_empty()) {
        return name.to_string();
    }
    match (
        user_agent.and_then(browser_name),
        user_agent.and_then(os_name),
    ) {
        (Some(browser), Some(os)) => format!("{browser} on {os}"),
        (Some(browser), None) => browser.to_string(),
        (None, Some(os)) => os.to_string(),
        (None, None) => UNKNOWN_CLIENT.to_string(),
    }
}

/// The browser family a `User-Agent` claims, or `None` when nothing matches.
///
/// Order is the whole algorithm, because every one of these strings lies about
/// the ones below it: Edge and Opera both claim `Chrome`, Chrome claims
/// `Safari`, and Firefox on iOS claims both.
fn browser_name(ua: &str) -> Option<&'static str> {
    let families: [(&str, &str); 10] = [
        ("Edg", "Edge"),
        ("OPR/", "Opera"),
        ("Opera", "Opera"),
        ("Vivaldi", "Vivaldi"),
        ("Firefox/", "Firefox"),
        ("FxiOS/", "Firefox"),
        ("CriOS/", "Chrome"),
        ("Chromium/", "Chromium"),
        ("Chrome/", "Chrome"),
        ("Safari/", "Safari"),
    ];
    families
        .into_iter()
        .find(|(needle, _)| ua.contains(needle))
        .map(|(_, name)| name)
}

/// The operating system a `User-Agent` claims, or `None` when nothing matches.
///
/// Same ordering trap: an iOS header says "like Mac OS X" and an Android one
/// says "Linux", so the specific platforms have to be tested first.
fn os_name(ua: &str) -> Option<&'static str> {
    let systems: [(&str, &str); 8] = [
        ("iPhone", "iOS"),
        ("iPad", "iPadOS"),
        ("Android", "Android"),
        ("CrOS", "ChromeOS"),
        ("Windows", "Windows"),
        ("Macintosh", "macOS"),
        ("Mac OS X", "macOS"),
        ("Linux", "Linux"),
    ];
    systems
        .into_iter()
        .find(|(needle, _)| ua.contains(needle))
        .map(|(_, name)| name)
}

#[cfg(test)]
mod tests;
