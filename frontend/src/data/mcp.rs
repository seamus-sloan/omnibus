//! Hosted `/mcp` endpoint toggle data access (#2314). Web hits the
//! admin-gated `/api/settings/mcp` REST pair via `gloo-net` (same-origin
//! cookie session); SSR builds get no-op stubs so the Settings card compiles
//! and hydrates with identical markup. No mobile surface.
#![cfg(not(feature = "mobile"))]

// Referenced fully-qualified inside the web-gated fns — a top-level import
// would be unused in the SSR-stub build.

/// GET `/api/settings/mcp` (web) — whether the hosted MCP endpoint is on.
#[cfg(feature = "web")]
pub async fn mcp_status() -> Result<bool, String> {
    use gloo_net::http::Request;
    let res = Request::get("/api/settings/mcp")
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !res.ok() {
        return Err(format!("mcp status failed: {}", res.status()));
    }
    res.json::<omnibus_shared::McpStatus>()
        .await
        .map(|s| s.enabled)
        .map_err(|e| e.to_string())
}

/// POST `/api/settings/mcp` (web) — admin enable/disable of the endpoint.
#[cfg(feature = "web")]
pub async fn set_mcp_enabled(enabled: bool) -> Result<(), String> {
    use gloo_net::http::Request;
    let res = Request::post("/api/settings/mcp")
        .json(&omnibus_shared::McpStatus { enabled })
        .map_err(|e| e.to_string())?
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if res.ok() {
        Ok(())
    } else {
        Err(format!("saving MCP setting failed: {}", res.status()))
    }
}

// ── SSR stubs (server feature, no web) ───────────────────────────

/// SSR stub — the card fetches only after the WASM client mounts.
#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn mcp_status() -> Result<bool, String> {
    Ok(false)
}

/// SSR stub — the card mutates only after the WASM client mounts.
#[cfg(all(feature = "server", not(feature = "web")))]
pub async fn set_mcp_enabled(_enabled: bool) -> Result<(), String> {
    Ok(())
}
