//! Long-lived API tokens (`omni_…` bearers): create, look up, list, revoke.
//! Unlike sessions they never idle- or absolutely expire — revocation is the
//! whole lifecycle — so they suit credentials pasted into an MCP client's
//! config. Only SHA-256(raw) is stored, same discipline as session tokens.

use sqlx::{Row, SqlitePool};

use super::token::{generate_token, hash_token};
use super::{build_user_from_joined_row, now_unix, AuthError, AuthResult, User};

/// Recognizable prefix on every raw API token. The auth path routes on it
/// cheaply: an `Authorization: Bearer` value starting with this is looked up
/// in `api_tokens`, anything else in `sessions` — so neither table is probed
/// for the other's credentials.
pub const API_TOKEN_PREFIX: &str = "omni_";

/// Upper bound on the user-supplied token name, mirroring
/// `MAX_DEVICE_NAME_CHARS` for devices.
pub const MAX_API_TOKEN_NAME_CHARS: usize = 100;

/// Defensive ceiling on `list_api_tokens_for_user`, mirroring
/// `LIST_SESSIONS_LIMIT`.
pub const LIST_API_TOKENS_LIMIT: i64 = 500;

/// Only write `last_used_at` if the stored value is older than this many
/// seconds — same write-amplification guard as sessions.
const API_TOKEN_TOUCH_THRESHOLD_SECS: i64 = 5 * 60;

/// One API token row, minus the hash. `last_used_at` is `None` until the
/// token first authenticates a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiToken {
    pub id: i64,
    pub user_id: i64,
    pub name: String,
    pub created_at: i64,
    pub last_used_at: Option<i64>,
    pub revoked_at: Option<i64>,
}

/// Returned from [`create_api_token`]. Callers must show `raw_token` to the
/// client exactly once — the server only keeps `SHA-256(raw_token)`.
#[derive(Debug)]
pub struct NewApiToken {
    pub token: ApiToken,
    pub raw_token: String,
}

/// Validate + trim a user-supplied token name.
fn validate_token_name(name: &str) -> AuthResult<&str> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.chars().count() > MAX_API_TOKEN_NAME_CHARS {
        return Err(AuthError::Validation(format!(
            "token name must be 1–{MAX_API_TOKEN_NAME_CHARS} characters"
        )));
    }
    Ok(trimmed)
}

/// Mint a new API token for `user_id`, returning the row and the raw secret.
pub async fn create_api_token(
    pool: &SqlitePool,
    user_id: i64,
    name: &str,
) -> AuthResult<NewApiToken> {
    let name = validate_token_name(name)?;
    let raw = format!("{API_TOKEN_PREFIX}{}", generate_token()?);
    let hash = hash_token(&raw);

    let row = sqlx::query(
        "INSERT INTO api_tokens (token_hash, user_id, name)
         VALUES (?, ?, ?)
         RETURNING id, user_id, name, created_at, last_used_at, revoked_at",
    )
    .bind(&hash)
    .bind(user_id)
    .bind(name)
    .fetch_one(pool)
    .await?;

    Ok(NewApiToken {
        token: row_to_api_token(&row),
        raw_token: raw,
    })
}

/// Resolve a raw `omni_…` token into `(User, ApiToken)`. Rejects revoked
/// tokens with [`AuthError::SessionNotFound`] (same variant as an unknown
/// session, so the wire never distinguishes the two). No expiry check by
/// design — revocation is the lifecycle. Updates `last_used_at`
/// opportunistically, rate-limited like the session touch.
pub async fn lookup_api_token(pool: &SqlitePool, raw_token: &str) -> AuthResult<(User, ApiToken)> {
    let hash = hash_token(raw_token);
    let row = sqlx::query(
        "SELECT t.id AS t_id, t.user_id, t.name, t.created_at, t.last_used_at, t.revoked_at,
                u.id AS u_id, u.username, u.is_admin, u.can_upload, u.can_edit, u.can_download,
                u.kindle_email, u.display_name, u.hidden_formats,
                EXISTS(SELECT 1 FROM user_avatars a WHERE a.user_id = u.id) AS has_avatar
         FROM api_tokens t JOIN users u ON u.id = t.user_id
         WHERE t.token_hash = ?",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?
    .ok_or(AuthError::SessionNotFound)?;

    let revoked_at: Option<i64> = row.get("revoked_at");
    if revoked_at.is_some() {
        return Err(AuthError::SessionNotFound);
    }

    let user = build_user_from_joined_row(&row);
    let token = ApiToken {
        id: row.get("t_id"),
        user_id: row.get("u_id"),
        name: row.get("name"),
        created_at: row.get("created_at"),
        last_used_at: row.get("last_used_at"),
        revoked_at,
    };

    let now = now_unix();
    if now - token.last_used_at.unwrap_or(0) >= API_TOKEN_TOUCH_THRESHOLD_SECS {
        touch_last_used(pool, token.id, now).await?;
    }

    // AC5: one durable trace per authenticated request tying it to the token,
    // so `omnibus.log` distinguishes API-token auth from session bearers.
    tracing::debug!(
        token_id = token.id,
        user_id = user.id,
        "request authenticated via API token"
    );

    Ok((user, token))
}

/// Write the current timestamp to `api_tokens.last_used_at`, guarded on
/// `revoked_at IS NULL` so a racing revoke can't have its row touched.
async fn touch_last_used(pool: &SqlitePool, token_id: i64, now: i64) -> AuthResult<()> {
    sqlx::query("UPDATE api_tokens SET last_used_at = ? WHERE id = ? AND revoked_at IS NULL")
        .bind(now)
        .bind(token_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// List `user_id`'s live (non-revoked) API tokens, newest first, up to
/// [`LIST_API_TOKENS_LIMIT`].
pub async fn list_api_tokens_for_user(
    pool: &SqlitePool,
    user_id: i64,
) -> AuthResult<Vec<ApiToken>> {
    let rows = sqlx::query(
        "SELECT id, user_id, name, created_at, last_used_at, revoked_at
         FROM api_tokens
         WHERE user_id = ? AND revoked_at IS NULL
         ORDER BY created_at DESC, id DESC
         LIMIT ?",
    )
    .bind(user_id)
    .bind(LIST_API_TOKENS_LIMIT)
    .fetch_all(pool)
    .await?;
    Ok(rows.iter().map(row_to_api_token).collect())
}

/// Revoke one of `user_id`'s API tokens by id, so [`lookup_api_token`]
/// rejects it immediately. Returns [`AuthError::ApiTokenNotFound`] when the
/// id is unknown, already revoked, or owned by a different user — the three
/// are indistinguishable on the wire by design.
pub async fn revoke_api_token_for_user(
    pool: &SqlitePool,
    user_id: i64,
    token_id: i64,
) -> AuthResult<()> {
    let r = sqlx::query(
        "UPDATE api_tokens SET revoked_at = ?
         WHERE id = ? AND user_id = ? AND revoked_at IS NULL",
    )
    .bind(now_unix())
    .bind(token_id)
    .bind(user_id)
    .execute(pool)
    .await?;
    if r.rows_affected() == 0 {
        return Err(AuthError::ApiTokenNotFound);
    }
    Ok(())
}

fn row_to_api_token(row: &sqlx::sqlite::SqliteRow) -> ApiToken {
    ApiToken {
        id: row.get("id"),
        user_id: row.get("user_id"),
        name: row.get("name"),
        created_at: row.get("created_at"),
        last_used_at: row.get("last_used_at"),
        revoked_at: row.get("revoked_at"),
    }
}

#[cfg(test)]
mod tests;
