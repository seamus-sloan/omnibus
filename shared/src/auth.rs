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

/// Stands in for a session whose holder can't be named — one minted before
/// migration `0088`, or a caller that sent no `User-Agent`. Lives beside the
/// wire type rather than beside the labelling logic in `server::auth` because
/// [`SessionView::client`]'s serde default needs it too, and one literal can't
/// drift from itself.
pub const UNKNOWN_CLIENT: &str = "Unknown client";

/// Serde default for [`SessionView::client`]. A bare `#[serde(default)]` would
/// decode a payload without the field to `""` — a blank cell in the UI, and a
/// contradiction of the field's own contract.
fn unknown_client() -> String {
    UNKNOWN_CLIENT.to_string()
}

/// A session row as shown to its owner or an admin (device & session
/// management, F5.4). Never exposes the token hash — only enough to
/// identify and revoke the row. Shared by the self-service
/// `GET /api/auth/sessions` and the admin `GET /api/admin/users/{id}/sessions`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SessionView {
    pub id: i64,
    #[serde(default)]
    pub device_id: Option<i64>,
    /// `"cookie"` or `"bearer"`.
    pub kind: String,
    /// Human-readable name of the client holding this session — the registered
    /// device's name for a native client ("Seamus's iPhone"), otherwise a label
    /// derived from the captured `User-Agent` ("Firefox on macOS"). Derived
    /// server-side so the raw header never reaches a page; falls back to
    /// [`UNKNOWN_CLIENT`] for a session minted before migration `0088`.
    #[serde(default = "unknown_client")]
    pub client: String,
    pub created_at: i64,
    pub last_used_at: i64,
    pub expires_at: i64,
    /// `true` when this is the session the request authenticated with. Only
    /// ever set by the self-service listing (`GET /api/auth/sessions`); the
    /// admin listing (`GET /api/admin/users/{id}/sessions`) always returns
    /// `false` here — including when an admin lists their own account, where
    /// the viewed row *could* be the admin's own request session — because
    /// that endpoint's builder never checks the requester's own session id
    /// against the row. Enforcement lives on the write side, not this flag:
    /// the self-service `DELETE /api/auth/sessions/{id}` refuses (400) to
    /// revoke the id the request itself authenticated with (AC2), regardless
    /// of whether a listing ever marked it current.
    #[serde(default)]
    pub is_current: bool,
}

/// A registered device row as shown to its owner or an admin. Shared by the
/// admin `GET /api/admin/users/{id}/devices` listing (self-service devices
/// are deferred — see `/api/auth/devices` note in the PR body).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct DeviceView {
    pub id: i64,
    pub name: String,
    pub client_kind: String,
    #[serde(default)]
    pub client_version: Option<String>,
    pub created_at: i64,
    pub last_seen_at: i64,
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

/// One live API token in the management listing (`GET /api/auth/api-tokens`).
/// Never carries the secret — that appears exactly once, in
/// [`CreateApiTokenResponse::secret`].
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ApiTokenView {
    pub id: i64,
    pub name: String,
    pub created_at: i64,
    /// `None` until the token first authenticates a request.
    #[serde(default)]
    pub last_used_at: Option<i64>,
    /// Last 4 characters of the raw token, for the `omni_…xxxx` display
    /// identifier. `None` on tokens minted before the suffix was recorded.
    #[serde(default)]
    pub suffix: Option<String>,
}

/// Body of `POST /api/auth/api-tokens`.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateApiTokenRequest {
    pub name: String,
}

/// Body of `PATCH /api/auth/api-tokens/{id}` — the row's Rename action.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RenameApiTokenRequest {
    pub name: String,
}

/// Response from `POST /api/auth/api-tokens`. `secret` is the raw `omni_…`
/// token, shown to the client exactly once — the server keeps only its hash,
/// so it is unrecoverable afterward. Deliberately does not derive `Debug`,
/// same as `LoginRequest`: a stray `tracing::debug!(?resp)` must not leak a
/// long-lived credential.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CreateApiTokenResponse {
    pub token: ApiTokenView,
    pub secret: String,
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

    // Payloads from a pre-0088 server lack `client`. The default must name the
    // absence rather than leaving `""`, which would render a blank cell where
    // the field's contract promises a client name.
    #[test]
    fn session_view_deserializes_payload_missing_client_as_unknown() {
        let v: SessionView = serde_json::from_str(
            r#"{"id":1,"kind":"cookie","created_at":0,
                "last_used_at":0,"expires_at":0}"#,
        )
        .unwrap();
        assert_eq!(v.client, UNKNOWN_CLIENT);
    }
}
