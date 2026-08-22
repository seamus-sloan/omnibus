//! Admin user-management client wrappers. Web hits the AdminUser-gated
//! `/api/users*` REST routes via `gloo-net` (same-origin cookie session,
//! mirroring `data::auth`); SSR builds get no-op stubs so the Users settings
//! page compiles and hydrates with identical markup. Mobile has no admin
//! Users surface, so this module is not compiled there.

use omnibus_shared::{AdminUserRow, CreateUserRequest, UserPermissions};

// ── Web (gloo-net) ───────────────────────────────────────────────

/// GET `/api/users` — list all users for the admin table.
#[cfg(feature = "web")]
pub async fn list_users() -> Result<Vec<AdminUserRow>, String> {
    use gloo_net::http::Request;
    let res = Request::get("/api/users")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(status_error(res).await);
    }
    res.json::<Vec<AdminUserRow>>()
        .await
        .map_err(|e| e.to_string())
}

/// POST `/api/users` — create a user; returns the created row.
#[cfg(feature = "web")]
pub async fn create_user(req: CreateUserRequest) -> Result<AdminUserRow, String> {
    use gloo_net::http::Request;
    let res = Request::post("/api/users")
        .json(&req)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(status_error(res).await);
    }
    res.json::<AdminUserRow>().await.map_err(|e| e.to_string())
}

/// PATCH `/api/users/{id}/permissions` — replace the four permission flags.
#[cfg(feature = "web")]
pub async fn update_permissions(id: i64, perms: UserPermissions) -> Result<(), String> {
    use gloo_net::http::Request;
    let res = Request::patch(&format!("/api/users/{id}/permissions"))
        .json(&perms)
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    ok_or_error(res).await
}

/// POST `/api/users/{id}/password` — admin password reset.
#[cfg(feature = "web")]
pub async fn set_password(id: i64, password: String) -> Result<(), String> {
    use gloo_net::http::Request;
    let res = Request::post(&format!("/api/users/{id}/password"))
        .json(&omnibus_shared::SetPasswordRequest { password })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    ok_or_error(res).await
}

/// POST `/api/users/{id}/unlock` — clear a failed-login lockout.
#[cfg(feature = "web")]
pub async fn unlock_user(id: i64) -> Result<(), String> {
    use gloo_net::http::Request;
    let res = Request::post(&format!("/api/users/{id}/unlock"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    ok_or_error(res).await
}

/// DELETE `/api/users/{id}` — delete a user (their owned rows cascade).
#[cfg(feature = "web")]
pub async fn delete_user(id: i64) -> Result<(), String> {
    use gloo_net::http::Request;
    let res = Request::delete(&format!("/api/users/{id}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    ok_or_error(res).await
}

/// Read the response body as the error message, falling back to the status
/// code. The `/api/users*` handlers return a plain-text reason (e.g.
/// "username is already taken", "cannot delete the last administrator") that
/// the Users UI surfaces inline.
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
// The Users page renders on the server too (hydration parity); these keep it
// compiling. They never run — the page's data effects only fire after the
// WASM client mounts.

/// SSR stub — the Users page fetches only after the WASM client mounts.
#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn list_users() -> Result<Vec<AdminUserRow>, String> {
    Ok(Vec::new())
}

/// SSR stub; fails loudly, since nothing should reach it during SSR.
#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn create_user(_req: CreateUserRequest) -> Result<AdminUserRow, String> {
    Err("unavailable".to_string())
}

/// SSR stub — the Users page mutates only after the WASM client mounts.
#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn update_permissions(_id: i64, _perms: UserPermissions) -> Result<(), String> {
    Ok(())
}

/// SSR stub — the Users page mutates only after the WASM client mounts.
#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn set_password(_id: i64, _password: String) -> Result<(), String> {
    Ok(())
}

/// SSR stub — the Users page mutates only after the WASM client mounts.
#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn unlock_user(_id: i64) -> Result<(), String> {
    Ok(())
}

/// SSR stub — the Users page mutates only after the WASM client mounts.
#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn delete_user(_id: i64) -> Result<(), String> {
    Ok(())
}
