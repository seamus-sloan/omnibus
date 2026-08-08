//! Admin device & session management client wrappers (F5.4, #910). Web hits
//! the AdminUser-gated `/api/admin/*` REST routes via `gloo-net` (same-origin
//! cookie session, mirroring `data::users`); SSR builds get no-op stubs so
//! the Users settings page compiles and hydrates with identical markup.
//! Mobile has no admin surface, so this module is not compiled there.

use omnibus_shared::{DeviceView, SessionView};

// ── Web (gloo-net) ───────────────────────────────────────────────

/// GET `/api/admin/users/{id}/sessions` — list a target user's live sessions.
#[cfg(feature = "web")]
pub async fn list_user_sessions(user_id: i64) -> Result<Vec<SessionView>, String> {
    use gloo_net::http::Request;
    let res = Request::get(&format!("/api/admin/users/{user_id}/sessions"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(status_error(res).await);
    }
    res.json::<Vec<SessionView>>()
        .await
        .map_err(|e| e.to_string())
}

/// GET `/api/admin/users/{id}/devices` — list a target user's registered devices.
#[cfg(feature = "web")]
pub async fn list_user_devices(user_id: i64) -> Result<Vec<DeviceView>, String> {
    use gloo_net::http::Request;
    let res = Request::get(&format!("/api/admin/users/{user_id}/devices"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(status_error(res).await);
    }
    res.json::<Vec<DeviceView>>()
        .await
        .map_err(|e| e.to_string())
}

/// DELETE `/api/admin/sessions/{id}` — revoke any user's session.
#[cfg(feature = "web")]
pub async fn revoke_user_session(session_id: i64) -> Result<(), String> {
    use gloo_net::http::Request;
    let res = Request::delete(&format!("/api/admin/sessions/{session_id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    ok_or_error(res).await
}

/// DELETE `/api/admin/devices/{id}` — revoke any user's device registration
/// (and any session still tied to it).
#[cfg(feature = "web")]
pub async fn revoke_user_device(device_id: i64) -> Result<(), String> {
    use gloo_net::http::Request;
    let res = Request::delete(&format!("/api/admin/devices/{device_id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    ok_or_error(res).await
}

/// Read the response body as the error message, falling back to the status
/// code. Mirrors `data::users::status_error`.
#[cfg(feature = "web")]
async fn status_error(res: gloo_net::http::Response) -> String {
    let status = res.status();
    let body = res.text().await.unwrap_or_default();
    if body.trim().is_empty() {
        format!("request failed ({status})")
    } else {
        body
    }
}

/// Collapse a no-content mutation response into `Result<(), String>`.
#[cfg(feature = "web")]
async fn ok_or_error(res: gloo_net::http::Response) -> Result<(), String> {
    if res.ok() {
        Ok(())
    } else {
        Err(status_error(res).await)
    }
}

// ── SSR stubs (server feature, no web) ───────────────────────────
//
// The Sessions modal renders on the server too (hydration parity); these
// keep it compiling. They never run — the modal's data effects only fire
// after the WASM client mounts.

#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn list_user_sessions(_user_id: i64) -> Result<Vec<SessionView>, String> {
    Ok(Vec::new())
}

#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn list_user_devices(_user_id: i64) -> Result<Vec<DeviceView>, String> {
    Ok(Vec::new())
}

#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn revoke_user_session(_session_id: i64) -> Result<(), String> {
    Ok(())
}

#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn revoke_user_device(_device_id: i64) -> Result<(), String> {
    Ok(())
}
