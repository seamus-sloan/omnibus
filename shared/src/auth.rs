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
    /// Send-to-Kindle destination address, or `None` when the user hasn't
    /// configured one. Non-secret — the user set it themselves.
    #[serde(default)]
    pub kindle_email: Option<String>,
    /// Presentation name other users see, or `None` to fall back to
    /// [`Self::username`] — which stays the login and admin identity.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Whether this user has uploaded an avatar. Clients render
    /// `GET /api/users/{id}/avatar` when set and a monogram otherwise.
    #[serde(default)]
    pub has_avatar: bool,
    /// Formats this user hides from the landing All Books view — canonical
    /// lowercase tokens (`"cbz"`). Clients pass it back as the listing
    /// request's `exclude_formats`; empty means nothing hidden.
    #[serde(default)]
    pub hidden_formats: Vec<String>,
}

impl UserSummary {
    /// The name to show for this user: their display name when set, else the
    /// username. Every user-facing surface should render this, never
    /// [`Self::username`] on its own.
    pub fn display(&self) -> &str {
        self.display_name.as_deref().unwrap_or(&self.username)
    }
}

/// The four permission booleans that define what a user can do. `is_admin`
/// is presented in the UI as an "Administrator" permission that implies the
/// other three; the storage layer keeps them as independent flags (there is
/// no role enum). Used by the admin create/edit endpoints (F5.4).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct UserPermissions {
    pub is_admin: bool,
    pub can_upload: bool,
    pub can_edit: bool,
    pub can_download: bool,
}

/// Admin Users-table projection of a `users` row (F5.4). Extends
/// [`UserSummary`] with the created timestamp and locked state the admin
/// table renders, without bloating the login-path summary. No password
/// fields ever cross the wire.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct AdminUserRow {
    pub id: i64,
    pub username: String,
    pub is_admin: bool,
    pub can_upload: bool,
    pub can_edit: bool,
    pub can_download: bool,
    #[serde(default)]
    pub kindle_email: Option<String>,
    /// Presentation name, or `None` when the user hasn't set one.
    #[serde(default)]
    pub display_name: Option<String>,
    /// Account creation time (Unix seconds).
    pub created_at: i64,
    /// `true` when the account is currently locked out by repeated failed
    /// logins (`locked_until` is in the future).
    pub locked: bool,
}

/// Request body for `POST /api/users` (admin create). See [`LoginRequest`]
/// for why `Debug` is deliberately not derived.
#[derive(Clone, Serialize, Deserialize)]
pub struct CreateUserRequest {
    pub username: String,
    pub password: String,
    pub permissions: UserPermissions,
}

/// Request body for `POST /api/users/{id}/password` (admin password reset).
/// See [`LoginRequest`] for why `Debug` is deliberately not derived.
#[derive(Clone, Serialize, Deserialize)]
pub struct SetPasswordRequest {
    pub password: String,
}

/// Whether self-registration is open; the body of `GET /api/auth/registration`
/// and `POST /api/settings/registration`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct RegistrationStatus {
    pub enabled: bool,
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

#[cfg(test)]
mod tests {
    use super::*;

    // Payloads from a pre-0068 server lack `hidden_formats`; the serde
    // default must keep them decoding (old cached `/me` bodies included).
    #[test]
    fn user_summary_deserializes_payload_missing_hidden_formats() {
        let v: UserSummary = serde_json::from_str(
            r#"{"id":1,"username":"alice","is_admin":false,
                "can_upload":false,"can_edit":false,"can_download":true}"#,
        )
        .unwrap();
        assert!(v.hidden_formats.is_empty());
    }
}
