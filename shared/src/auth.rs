//! Auth wire types shared between the server and both clients.
//!
//! Used by `/api/auth/{login,register,me}` and the mobile bearer-token flow.
//! `LoginRequest` / `RegisterRequest` deliberately do not derive `Debug` so a
//! stray `tracing::debug!(?req)` cannot leak plaintext passwords.

use serde::{Deserialize, Serialize};

/// Safe projection of a `users` row. No password fields ever cross the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserSummary {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub can_upload: bool,
    pub can_edit: bool,
    pub can_download: bool,
    /// F4.3 Send-to-Kindle destination address, or `None` when the user hasn't
    /// configured one. Non-secret — the user set it themselves.
    #[serde(default)]
    pub kindle_email: Option<String>,
}

/// Request body for `POST /api/auth/login`.
///
/// Deliberately does not derive `Debug`: the struct holds a plaintext
/// password, and a stray `tracing::debug!(?req)` would write it to logs.
#[derive(Clone, Serialize, Deserialize)]
pub struct LoginRequest {
    pub username: String,
    pub password: String,
    /// Optional — when present, server issues a bearer session instead of a
    /// cookie session and includes the raw token in the response.
    #[serde(default)]
    pub client_kind: Option<String>,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub client_version: Option<String>,
}

/// Request body for `POST /api/auth/register`. See [`LoginRequest`] for
/// why `Debug` is deliberately not derived.
#[derive(Clone, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub username: String,
    pub password: String,
    #[serde(default)]
    pub client_kind: Option<String>,
    #[serde(default)]
    pub device_name: Option<String>,
    #[serde(default)]
    pub client_version: Option<String>,
}

/// Response from `POST /api/auth/login` and `POST /api/auth/register`.
/// `token` is populated only for bearer (mobile) sessions; cookie sessions
/// return the cookie in a `Set-Cookie` header and `token` is `None`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoginResponse {
    pub user: UserSummary,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
}
