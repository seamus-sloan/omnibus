//! Sessions.

use sqlx::{Executor, Row, Sqlite, SqlitePool};

use super::token::{generate_token, hash_token, parse_session_token};
use super::{now_unix, AuthError, AuthResult, NewSession, Session, SessionKind, User};

/// Only write `last_used_at` if the existing value is older than this many
/// seconds. Avoids write-amplification on every authenticated request.
const SESSION_TOUCH_THRESHOLD_SECS: i64 = 5 * 60;

/// Idle-expiry threshold. A session whose `last_used_at` is older than this
/// is treated as expired by `lookup_session`, even if `expires_at` is still
/// in the future. Caps the blast radius of a stolen or forgotten session
/// (cookie absolute TTL is 30 days; bearer is 90).
pub(crate) const SESSION_IDLE_TIMEOUT_SECS: i64 = 7 * 24 * 60 * 60;

/// Create a new session for `user_id`, returning the session record and the raw (unhashed) token.
///
/// Accepts any `sqlx::Executor` so callers can pass either a `&SqlitePool` for
/// a standalone insert or `&mut *tx` from within a transaction — the login
/// path in `server::auth::handlers::issue_session` uses the latter to keep the
/// device + session inserts atomic (see #627).
pub async fn create_session<'e, E>(
    executor: E,
    user_id: i64,
    device_id: Option<i64>,
    kind: SessionKind,
    ttl_secs: i64,
) -> AuthResult<NewSession>
where
    E: Executor<'e, Database = Sqlite>,
{
    let raw = generate_token()?;
    let hash = hash_token(&raw);
    let now = now_unix();
    let expires = now + ttl_secs;

    let row = sqlx::query(
        "INSERT INTO sessions (token_hash, user_id, device_id, kind, expires_at)
         VALUES (?, ?, ?, ?, ?)
         RETURNING id, user_id, device_id, kind, created_at, last_used_at, expires_at",
    )
    .bind(&hash)
    .bind(user_id)
    .bind(device_id)
    .bind(kind.as_str())
    .bind(expires)
    .fetch_one(executor)
    .await?;

    let session = Session {
        id: row.get("id"),
        user_id: row.get("user_id"),
        device_id: row.get("device_id"),
        kind,
        created_at: row.get("created_at"),
        last_used_at: row.get("last_used_at"),
        expires_at: row.get("expires_at"),
    };

    Ok(NewSession {
        session,
        raw_token: raw,
    })
}

/// Resolve a raw token into `(User, Session)`. Rejects expired or revoked
/// sessions. Updates `last_used_at` opportunistically (rate-limited by
/// `SESSION_TOUCH_THRESHOLD_SECS`).
pub async fn lookup_session(pool: &SqlitePool, raw_token: &str) -> AuthResult<(User, Session)> {
    let hash = hash_token(raw_token);
    let now = now_unix();

    let row = fetch_session_row(pool, &hash).await?;
    check_session_validity(&row, now)?;

    let session_id: i64 = row.get("s_id");
    let last_used_at: i64 = row.get("last_used_at");

    let user = build_user_from_row(&row);
    let session = build_session_from_row(&row, session_id, last_used_at)?;

    if now - last_used_at >= SESSION_TOUCH_THRESHOLD_SECS {
        touch_last_used(pool, session_id, now).await?;
    }

    Ok((user, session))
}

/// Fetch the joined session+user row for a token hash. Returns `SessionNotFound`
/// when no row matches — the caller treats any absence as an invalid token.
async fn fetch_session_row(pool: &SqlitePool, hash: &[u8]) -> AuthResult<sqlx::sqlite::SqliteRow> {
    let row = sqlx::query(
        "SELECT s.id AS s_id, s.user_id, s.device_id, s.kind, s.created_at,
                s.last_used_at, s.expires_at, s.revoked_at,
                u.id AS u_id, u.username, u.is_admin, u.can_upload, u.can_edit, u.can_download,
                u.kindle_email, u.display_name,
                EXISTS(SELECT 1 FROM user_avatars a WHERE a.user_id = u.id) AS has_avatar
         FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = ?",
    )
    .bind(hash)
    .fetch_optional(pool)
    .await?;

    row.ok_or(AuthError::SessionNotFound)
}

/// Reject a session row that is revoked, absolutely expired, or idle-expired.
/// Returns `SessionNotFound` for any disqualifying condition so callers see
/// one error variant and the specific reason isn't leaked to the wire.
fn check_session_validity(row: &sqlx::sqlite::SqliteRow, now: i64) -> AuthResult<()> {
    let revoked_at: Option<i64> = row.get("revoked_at");
    let expires_at: i64 = row.get("expires_at");
    if revoked_at.is_some() || expires_at <= now {
        return Err(AuthError::SessionNotFound);
    }

    let last_used_at: i64 = row.get("last_used_at");
    // Idle expiry: a session that hasn't been touched in
    // `SESSION_IDLE_TIMEOUT_SECS` is treated as expired regardless of its
    // absolute `expires_at`. `last_used_at` is updated opportunistically
    // below (rate-limited by `SESSION_TOUCH_THRESHOLD_SECS`), so this
    // genuinely tracks user inactivity.
    if now - last_used_at > SESSION_IDLE_TIMEOUT_SECS {
        return Err(AuthError::SessionNotFound);
    }

    Ok(())
}

/// Build a `User` from the joined session+user query row.
fn build_user_from_row(row: &sqlx::sqlite::SqliteRow) -> User {
    User {
        id: row.get("u_id"),
        username: row.get("username"),
        is_admin: row.get::<i64, _>("is_admin") != 0,
        can_upload: row.get::<i64, _>("can_upload") != 0,
        can_edit: row.get::<i64, _>("can_edit") != 0,
        can_download: row.get::<i64, _>("can_download") != 0,
        kindle_email: row.get("kindle_email"),
        display_name: row.get("display_name"),
        has_avatar: row.get::<i64, _>("has_avatar") != 0,
    }
}

/// Build a `Session` from the joined query row. Returns `SessionNotFound` if
/// the `kind` column holds an unrecognised value — the migration enforces the
/// CHECK constraint, so an unknown kind means DB corruption or a hand-edited row.
fn build_session_from_row(
    row: &sqlx::sqlite::SqliteRow,
    session_id: i64,
    last_used_at: i64,
) -> AuthResult<Session> {
    let expires_at: i64 = row.get("expires_at");
    let kind_str: String = row.get("kind");
    let kind = match kind_str.as_str() {
        "cookie" => SessionKind::Cookie,
        "bearer" => SessionKind::Bearer,
        _ => return Err(AuthError::SessionNotFound),
    };
    Ok(Session {
        id: session_id,
        user_id: row.get("u_id"),
        device_id: row.get("device_id"),
        kind,
        created_at: row.get("created_at"),
        last_used_at,
        expires_at,
    })
}

/// Write the current timestamp to `sessions.last_used_at` for `session_id`,
/// guarded by `revoked_at IS NULL AND expires_at > now` so a session revoked
/// or expired between the outer SELECT and this UPDATE can't have its
/// activity timestamp bumped by a racing lookup. A 0-row result is benign —
/// the threshold guard in `lookup_session` controls whether we write at all.
async fn touch_last_used(pool: &SqlitePool, session_id: i64, now: i64) -> AuthResult<()> {
    // Re-assert validity inside the UPDATE WHERE clause so a session that is
    // revoked or (absolute-)expired between the SELECT above and this write
    // can't have its `last_used_at` bumped by a racing lookup.
    sqlx::query(
        "UPDATE sessions SET last_used_at = ?
         WHERE id = ? AND revoked_at IS NULL AND expires_at > ?",
    )
    .bind(now)
    .bind(session_id)
    .bind(now)
    .execute(pool)
    .await?;
    Ok(())
}

/// Outcome of [`validate_session`], collapsed to the only two dispositions
/// an HTTP auth extractor cares about:
///
/// * [`SessionAuthError::Unauthenticated`] — no usable token on the request,
///   or the token doesn't resolve to a live session (missing / expired /
///   idle-expired / revoked). HTTP callers map this to `401 Unauthorized`.
/// * [`SessionAuthError::Internal`] — an infrastructure failure (e.g. the DB
///   query itself errored). HTTP callers map this to `500`.
///
/// Keeping this enum here (rather than re-deriving the 401-vs-500 split in
/// each extractor) is the whole point of the consolidation: the
/// cookie/bearer → live-session contract lives in exactly one place.
#[derive(Debug, thiserror::Error)]
pub enum SessionAuthError {
    #[error("unauthenticated")]
    Unauthenticated,
    #[error(transparent)]
    Internal(AuthError),
}

/// Resolve an authenticated `(User, Session)` from raw HTTP header values,
/// preferring an `Authorization: Bearer …` token over the `omnibus_session`
/// cookie (see [`parse_session_token`]). This is the single consolidated
/// session-validation surface shared by every HTTP auth extractor — both the
/// REST `AuthUser`/`AdminUser` extractors (`server::auth::extractor`) and the
/// Dioxus server-function extractors (`omnibus_frontend::rpc`) delegate here
/// so the token precedence, SHA-256 hashing, absolute + idle expiry, and
/// revocation checks (all enforced by [`lookup_session`]) cannot drift between
/// the two code paths.
///
/// Pure-string API by design — `omnibus-db` stays free of axum/http types, so
/// each caller does only the thin work of pulling the header strings out of
/// its own request representation before delegating.
///
/// * `None` token → [`SessionAuthError::Unauthenticated`].
/// * [`AuthError::SessionNotFound`] → [`SessionAuthError::Unauthenticated`].
/// * any other [`AuthError`] → [`SessionAuthError::Internal`].
pub async fn validate_session(
    pool: &SqlitePool,
    authorization: Option<&str>,
    cookie_header: Option<&str>,
) -> Result<(User, Session), SessionAuthError> {
    let Some((token, _kind)) = parse_session_token(authorization, cookie_header) else {
        return Err(SessionAuthError::Unauthenticated);
    };
    match lookup_session(pool, &token).await {
        Ok(pair) => Ok(pair),
        Err(AuthError::SessionNotFound) => Err(SessionAuthError::Unauthenticated),
        Err(e) => Err(SessionAuthError::Internal(e)),
    }
}

/// Revoke a single session by id, setting its `revoked_at` timestamp so `lookup_session` rejects it.
pub async fn revoke_session(pool: &SqlitePool, session_id: i64) -> AuthResult<()> {
    sqlx::query("UPDATE sessions SET revoked_at = ? WHERE id = ?")
        .bind(now_unix())
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Revoke all active sessions for a user; returns the number of rows updated.
/// Accepts any `sqlx::Executor` so callers can run it inside an existing
/// transaction (e.g. alongside a password update, so a failure rolls back
/// both together) or standalone via `&SqlitePool`.
pub async fn revoke_all_sessions_for_user<'e, E>(executor: E, user_id: i64) -> AuthResult<u64>
where
    E: Executor<'e, Database = Sqlite>,
{
    let r =
        sqlx::query("UPDATE sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
            .bind(now_unix())
            .bind(user_id)
            .execute(executor)
            .await?;
    Ok(r.rows_affected())
}

/// Revoke all of a user's active sessions except `except_session_id`. Used by
/// the self-service password change so the caller's own session survives the
/// revocation sweep (they'd otherwise be logged out by the change they just
/// made). Accepts any `sqlx::Executor`, same rationale as
/// [`revoke_all_sessions_for_user`].
pub async fn revoke_all_sessions_for_user_except<'e, E>(
    executor: E,
    user_id: i64,
    except_session_id: i64,
) -> AuthResult<u64>
where
    E: Executor<'e, Database = Sqlite>,
{
    let r = sqlx::query(
        "UPDATE sessions SET revoked_at = ?
         WHERE user_id = ? AND id != ? AND revoked_at IS NULL",
    )
    .bind(now_unix())
    .bind(user_id)
    .bind(except_session_id)
    .execute(executor)
    .await?;
    Ok(r.rows_affected())
}

/// Hard-delete every session [`lookup_session`] would reject: revoked
/// (`revoked_at IS NOT NULL`), past its absolute `expires_at`, or idle-expired
/// (`last_used_at` older than [`SESSION_IDLE_TIMEOUT_SECS`]). Revocation marks
/// rows rather than deleting them (see `revoke_session`), so without this
/// prune the `sessions` table grows unboundedly — a permanent row per
/// login/logout that slows `lookup_session` and `revoke_all_sessions_for_user`.
///
/// The predicate is kept in lockstep with `lookup_session` (same `<=` absolute
/// boundary, same idle cutoff) so the table only ever retains rows that can
/// still authenticate. Returns the number of rows deleted; intended to run on
/// a periodic schedule (see the prune task in `server::main`).
///
/// Issued as three single-predicate DELETEs (one per index from migrations 0004
/// and 0012): SQLite scans the table for the OR'd form unless every branch is
/// indexed.
pub async fn prune_expired_sessions(pool: &SqlitePool) -> AuthResult<u64> {
    let now = now_unix();
    let idle_cutoff = now - SESSION_IDLE_TIMEOUT_SECS;

    // One transaction keeps the prune atomic; a row matching several predicates
    // is removed by the first matching DELETE, so the counts don't overlap.
    let mut tx = pool.begin().await?;
    let revoked = sqlx::query("DELETE FROM sessions WHERE revoked_at IS NOT NULL")
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let expired = sqlx::query("DELETE FROM sessions WHERE expires_at <= ?")
        .bind(now)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    let idle = sqlx::query("DELETE FROM sessions WHERE last_used_at < ?")
        .bind(idle_cutoff)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    Ok(revoked + expired + idle)
}

#[cfg(test)]
mod tests;
