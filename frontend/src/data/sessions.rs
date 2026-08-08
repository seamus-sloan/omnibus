//! Self-service session management client wrappers (F5.4, #910). Web hits
//! the `/api/auth/sessions*` REST routes via `gloo-net` (same-origin cookie
//! session); SSR builds get no-op stubs so the Account page compiles and
//! hydrates with identical markup. Mobile has no self-service surface yet
//! for this card (its "You" tab predates this feature; see #910's PR body),
//! so this module is not compiled there.

use omnibus_shared::SessionView;

// ── Web (gloo-net) ───────────────────────────────────────────────

/// GET `/api/auth/sessions` — list the caller's own live sessions.
#[cfg(feature = "web")]
pub async fn list_my_sessions() -> Result<Vec<SessionView>, String> {
    use gloo_net::http::Request;
    let res = Request::get("/api/auth/sessions")
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

/// DELETE `/api/auth/sessions/{id}` — revoke one of the caller's own
/// sessions (never the currently-active one; the server refuses that with
/// `400`).
#[cfg(feature = "web")]
pub async fn revoke_my_session(session_id: i64) -> Result<(), String> {
    use gloo_net::http::Request;
    let res = Request::delete(&format!("/api/auth/sessions/{session_id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if res.ok() {
        Ok(())
    } else {
        Err(status_error(res).await)
    }
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

// ── SSR stubs (server feature, no web) ───────────────────────────

#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn list_my_sessions() -> Result<Vec<SessionView>, String> {
    Ok(Vec::new())
}

#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn revoke_my_session(_session_id: i64) -> Result<(), String> {
    Ok(())
}
