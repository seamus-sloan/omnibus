//! Sessions.

use sqlx::{Row, SqlitePool};

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

pub async fn create_session(
    pool: &SqlitePool,
    user_id: i64,
    device_id: Option<i64>,
    kind: SessionKind,
    ttl_secs: i64,
) -> AuthResult<NewSession> {
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
    .fetch_one(pool)
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

    let row = sqlx::query(
        "SELECT s.id AS s_id, s.user_id, s.device_id, s.kind, s.created_at,
                s.last_used_at, s.expires_at, s.revoked_at,
                u.id AS u_id, u.username, u.is_admin, u.can_upload, u.can_edit, u.can_download
         FROM sessions s JOIN users u ON u.id = s.user_id
         WHERE s.token_hash = ?",
    )
    .bind(&hash)
    .fetch_optional(pool)
    .await?;

    let Some(row) = row else {
        return Err(AuthError::SessionNotFound);
    };

    let revoked_at: Option<i64> = row.get("revoked_at");
    let expires_at: i64 = row.get("expires_at");
    if revoked_at.is_some() || expires_at <= now {
        return Err(AuthError::SessionNotFound);
    }

    let session_id: i64 = row.get("s_id");
    let last_used_at: i64 = row.get("last_used_at");

    // Idle expiry: a session that hasn't been touched in
    // `SESSION_IDLE_TIMEOUT_SECS` is treated as expired regardless of its
    // absolute `expires_at`. `last_used_at` is updated opportunistically
    // below (rate-limited by `SESSION_TOUCH_THRESHOLD_SECS`), so this
    // genuinely tracks user inactivity.
    if now - last_used_at > SESSION_IDLE_TIMEOUT_SECS {
        return Err(AuthError::SessionNotFound);
    }

    let user = User {
        id: row.get("u_id"),
        username: row.get("username"),
        is_admin: row.get::<i64, _>("is_admin") != 0,
        can_upload: row.get::<i64, _>("can_upload") != 0,
        can_edit: row.get::<i64, _>("can_edit") != 0,
        can_download: row.get::<i64, _>("can_download") != 0,
    };

    let kind_str: String = row.get("kind");
    let kind = match kind_str.as_str() {
        "cookie" => SessionKind::Cookie,
        "bearer" => SessionKind::Bearer,
        // The migration enforces this via CHECK, so an unknown value here
        // means DB corruption or a hand-edited row. Fail closed rather
        // than silently apply the wrong semantics.
        _ => return Err(AuthError::SessionNotFound),
    };
    let session = Session {
        id: session_id,
        user_id: user.id,
        device_id: row.get("device_id"),
        kind,
        created_at: row.get("created_at"),
        last_used_at,
        expires_at,
    };

    if now - last_used_at >= SESSION_TOUCH_THRESHOLD_SECS {
        // Re-assert validity inside the touch UPDATE so a session that is
        // revoked or (absolute-)expired between the SELECT above and this
        // write can't have its `last_used_at` bumped by a racing lookup.
        // The threshold guard stays the gate for *whether* we touch, so this
        // remains rate-limited and a 0-row result here is benign.
        sqlx::query(
            "UPDATE sessions SET last_used_at = ?
             WHERE id = ? AND revoked_at IS NULL AND expires_at > ?",
        )
        .bind(now)
        .bind(session_id)
        .bind(now)
        .execute(pool)
        .await?;
    }

    Ok((user, session))
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

pub async fn revoke_session(pool: &SqlitePool, session_id: i64) -> AuthResult<()> {
    sqlx::query("UPDATE sessions SET revoked_at = ? WHERE id = ?")
        .bind(now_unix())
        .bind(session_id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn revoke_all_sessions_for_user(pool: &SqlitePool, user_id: i64) -> AuthResult<u64> {
    let r =
        sqlx::query("UPDATE sessions SET revoked_at = ? WHERE user_id = ? AND revoked_at IS NULL")
            .bind(now_unix())
            .bind(user_id)
            .execute(pool)
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
mod tests {
    use super::*;
    use crate::auth::test_support::pool;
    use crate::auth::token::SESSION_COOKIE_NAME;
    use crate::auth::users::create_user;

    #[tokio::test]
    async fn session_roundtrip() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
            .await
            .unwrap();
        let (user2, sess2) = lookup_session(&p, &ns.raw_token).await.unwrap();
        assert_eq!(user2.id, u.id);
        assert_eq!(sess2.id, ns.session.id);
        assert_eq!(sess2.kind, SessionKind::Cookie);
    }

    #[tokio::test]
    async fn session_lookup_hashes_token() {
        // Proves the db does not store the raw token: look up by the hash
        // directly and ensure NO row has the raw token as its hash column.
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
            .await
            .unwrap();
        let raw_as_hash: Option<i64> =
            sqlx::query_scalar("SELECT id FROM sessions WHERE token_hash = ?")
                .bind(ns.raw_token.as_bytes())
                .fetch_optional(&p)
                .await
                .unwrap();
        assert!(raw_as_hash.is_none(), "raw token must not be stored");
    }

    #[tokio::test]
    async fn expired_session_is_rejected() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
            .await
            .unwrap();
        // Simulate expiry by rewriting the row.
        sqlx::query("UPDATE sessions SET expires_at = 1 WHERE id = ?")
            .bind(ns.session.id)
            .execute(&p)
            .await
            .unwrap();
        let err = lookup_session(&p, &ns.raw_token).await.unwrap_err();
        assert!(matches!(err, AuthError::SessionNotFound));
    }

    #[tokio::test]
    async fn session_idle_expired_after_threshold() {
        // Absolute expiry is still in the future, but `last_used_at` is
        // older than `SESSION_IDLE_TIMEOUT_SECS` — must be rejected.
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Cookie, 30 * 24 * 60 * 60)
            .await
            .unwrap();
        let stale = now_unix() - SESSION_IDLE_TIMEOUT_SECS - 1;
        sqlx::query("UPDATE sessions SET last_used_at = ? WHERE id = ?")
            .bind(stale)
            .bind(ns.session.id)
            .execute(&p)
            .await
            .unwrap();
        let err = lookup_session(&p, &ns.raw_token).await.unwrap_err();
        assert!(matches!(err, AuthError::SessionNotFound));
    }

    #[tokio::test]
    async fn session_idle_just_below_threshold_is_accepted() {
        // A session touched right before the idle cutoff stays valid.
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Cookie, 30 * 24 * 60 * 60)
            .await
            .unwrap();
        let fresh = now_unix() - SESSION_IDLE_TIMEOUT_SECS + 60;
        sqlx::query("UPDATE sessions SET last_used_at = ? WHERE id = ?")
            .bind(fresh)
            .bind(ns.session.id)
            .execute(&p)
            .await
            .unwrap();
        let (user2, _) = lookup_session(&p, &ns.raw_token).await.unwrap();
        assert_eq!(user2.id, u.id);
    }

    #[tokio::test]
    async fn revoked_session_is_rejected() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Bearer, 3600)
            .await
            .unwrap();
        revoke_session(&p, ns.session.id).await.unwrap();
        let err = lookup_session(&p, &ns.raw_token).await.unwrap_err();
        assert!(matches!(err, AuthError::SessionNotFound));
    }

    #[tokio::test]
    async fn lookup_session_touches_last_used_when_past_threshold() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Cookie, 30 * 24 * 60 * 60)
            .await
            .unwrap();
        // Past the touch threshold but well within the idle window -> still valid.
        let stale = now_unix() - SESSION_TOUCH_THRESHOLD_SECS - 60;
        sqlx::query("UPDATE sessions SET last_used_at = ? WHERE id = ?")
            .bind(stale)
            .bind(ns.session.id)
            .execute(&p)
            .await
            .unwrap();
        lookup_session(&p, &ns.raw_token).await.unwrap();
        let after: i64 = sqlx::query_scalar("SELECT last_used_at FROM sessions WHERE id = ?")
            .bind(ns.session.id)
            .fetch_one(&p)
            .await
            .unwrap();
        assert!(
            after > stale,
            "a valid session past the touch threshold should have last_used_at bumped"
        );
    }

    #[tokio::test]
    async fn lookup_session_skips_touch_within_threshold() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Cookie, 30 * 24 * 60 * 60)
            .await
            .unwrap();
        // Touched recently (inside the threshold): lookup must not rewrite it.
        let recent = now_unix() - (SESSION_TOUCH_THRESHOLD_SECS / 2);
        sqlx::query("UPDATE sessions SET last_used_at = ? WHERE id = ?")
            .bind(recent)
            .bind(ns.session.id)
            .execute(&p)
            .await
            .unwrap();
        lookup_session(&p, &ns.raw_token).await.unwrap();
        let after: i64 = sqlx::query_scalar("SELECT last_used_at FROM sessions WHERE id = ?")
            .bind(ns.session.id)
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(
            after, recent,
            "a recently-touched session must not be touched again (rate-limit preserved)"
        );
    }

    #[tokio::test]
    async fn touch_update_does_not_bump_revoked_session() {
        // #246: the opportunistic touch re-asserts validity in its UPDATE WHERE
        // clause so a session revoked/expired between the read and the write (a
        // concurrent-request race) can't have last_used_at bumped. The public
        // lookup rejects such sessions before the touch, so this exercises the
        // exact guarded statement lookup_session issues.
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Bearer, 3600)
            .await
            .unwrap();
        revoke_session(&p, ns.session.id).await.unwrap();
        let before: i64 = sqlx::query_scalar("SELECT last_used_at FROM sessions WHERE id = ?")
            .bind(ns.session.id)
            .fetch_one(&p)
            .await
            .unwrap();
        // Touch timestamp inside the session lifetime (created with TTL 3600)
        // and distinct from the creation-time last_used_at, so the
        // `expires_at > ?` guard passes — leaving `revoked_at IS NULL` as the
        // only reason the UPDATE matches 0 rows.
        let touch_at = now_unix() + 100;
        let touched = sqlx::query(
            "UPDATE sessions SET last_used_at = ?
             WHERE id = ? AND revoked_at IS NULL AND expires_at > ?",
        )
        .bind(touch_at)
        .bind(ns.session.id)
        .bind(touch_at)
        .execute(&p)
        .await
        .unwrap()
        .rows_affected();
        assert_eq!(touched, 0, "revoked session must not be touched");
        let after: i64 = sqlx::query_scalar("SELECT last_used_at FROM sessions WHERE id = ?")
            .bind(ns.session.id)
            .fetch_one(&p)
            .await
            .unwrap();
        assert_eq!(
            after, before,
            "revoked session last_used_at must be unchanged"
        );
    }

    #[tokio::test]
    async fn unknown_token_is_rejected() {
        let p = pool().await;
        let err = lookup_session(&p, "not-a-real-token").await.unwrap_err();
        assert!(matches!(err, AuthError::SessionNotFound));
    }

    #[tokio::test]
    async fn validate_session_resolves_bearer_token() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Bearer, 3600)
            .await
            .unwrap();
        let auth = format!("Bearer {}", ns.raw_token);
        let (user, sess) = validate_session(&p, Some(&auth), None).await.unwrap();
        assert_eq!(user.id, u.id);
        assert_eq!(sess.id, ns.session.id);
        assert_eq!(sess.kind, SessionKind::Bearer);
    }

    #[tokio::test]
    async fn validate_session_resolves_cookie_token() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
            .await
            .unwrap();
        let cookie = format!("{}={}", SESSION_COOKIE_NAME, ns.raw_token);
        let (user, sess) = validate_session(&p, None, Some(&cookie)).await.unwrap();
        assert_eq!(user.id, u.id);
        assert_eq!(sess.kind, SessionKind::Cookie);
    }

    #[tokio::test]
    async fn validate_session_no_token_is_unauthenticated() {
        let p = pool().await;
        let err = validate_session(&p, None, None).await.unwrap_err();
        assert!(matches!(err, SessionAuthError::Unauthenticated));
    }

    #[tokio::test]
    async fn validate_session_unknown_token_is_unauthenticated() {
        // A syntactically valid token that resolves to no live session maps
        // to Unauthenticated (401), not Internal (500).
        let p = pool().await;
        let err = validate_session(&p, Some("Bearer not-a-real-token"), None)
            .await
            .unwrap_err();
        assert!(matches!(err, SessionAuthError::Unauthenticated));
    }

    #[tokio::test]
    async fn validate_session_expired_is_unauthenticated() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let ns = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET expires_at = 1 WHERE id = ?")
            .bind(ns.session.id)
            .execute(&p)
            .await
            .unwrap();
        let cookie = format!("{}={}", SESSION_COOKIE_NAME, ns.raw_token);
        let err = validate_session(&p, None, Some(&cookie)).await.unwrap_err();
        assert!(matches!(err, SessionAuthError::Unauthenticated));
    }

    #[tokio::test]
    async fn validate_session_prefers_bearer_over_cookie() {
        // Mirrors parse_session_token precedence: a Bearer header wins even
        // when a (different) cookie is also present.
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
        let bearer = create_session(&p, u.id, None, SessionKind::Bearer, 3600)
            .await
            .unwrap();
        let cookie_sess = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
            .await
            .unwrap();
        let auth = format!("Bearer {}", bearer.raw_token);
        let cookie = format!("{}={}", SESSION_COOKIE_NAME, cookie_sess.raw_token);
        let (_user, sess) = validate_session(&p, Some(&auth), Some(&cookie))
            .await
            .unwrap();
        assert_eq!(sess.id, bearer.session.id);
        assert_eq!(sess.kind, SessionKind::Bearer);
    }

    #[tokio::test]
    async fn prune_removes_expired_revoked_and_idle_keeps_live() {
        let p = pool().await;
        let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

        // (a) Expired: still un-revoked, but past its absolute expiry.
        let expired = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET expires_at = 1 WHERE id = ?")
            .bind(expired.session.id)
            .execute(&p)
            .await
            .unwrap();

        // (b) Revoked: still within its expiry window, but soft-revoked.
        let revoked = create_session(&p, u.id, None, SessionKind::Bearer, 3600)
            .await
            .unwrap();
        revoke_session(&p, revoked.session.id).await.unwrap();

        // (c) Idle-expired: un-revoked and inside its absolute window, but
        // `last_used_at` is older than SESSION_IDLE_TIMEOUT_SECS, so
        // lookup_session would reject it. The prune must match that.
        let idle = create_session(&p, u.id, None, SessionKind::Cookie, 30 * 24 * 60 * 60)
            .await
            .unwrap();
        sqlx::query("UPDATE sessions SET last_used_at = 1 WHERE id = ?")
            .bind(idle.session.id)
            .execute(&p)
            .await
            .unwrap();

        // (d) Live: un-revoked, inside its absolute window, recently used.
        let live = create_session(&p, u.id, None, SessionKind::Cookie, 30 * 24 * 60 * 60)
            .await
            .unwrap();

        let deleted = prune_expired_sessions(&p).await.unwrap();
        assert_eq!(
            deleted, 3,
            "expired + revoked + idle rows should be deleted"
        );

        let remaining: Vec<i64> = sqlx::query_scalar("SELECT id FROM sessions ORDER BY id")
            .fetch_all(&p)
            .await
            .unwrap();
        assert_eq!(
            remaining,
            vec![live.session.id],
            "only the live session should survive the prune"
        );

        // Idempotent: a second prune with nothing to remove deletes zero rows.
        let again = prune_expired_sessions(&p).await.unwrap();
        assert_eq!(again, 0);
    }

    /// #248: the idle DELETE must use `idx_sessions_last_used_at`, not scan.
    /// Guards against regressing to the OR'd form (migration 0012).
    #[tokio::test]
    async fn prune_idle_delete_uses_last_used_index() {
        use sqlx::Row;
        let p = pool().await;
        let plan: String =
            sqlx::query("EXPLAIN QUERY PLAN DELETE FROM sessions WHERE last_used_at < ?")
                .bind(0_i64)
                .fetch_all(&p)
                .await
                .unwrap()
                .iter()
                .map(|r| r.get::<String, _>("detail"))
                .collect::<Vec<_>>()
                .join("\n");
        assert!(
            plan.contains("idx_sessions_last_used_at"),
            "idle-prune delete should use idx_sessions_last_used_at, got plan:\n{plan}"
        );
    }

    /// #248: the revoked DELETE must use the partial `idx_sessions_revoked_at`,
    /// not scan. Guards against regressing to the OR'd form (migration 0012).
    #[tokio::test]
    async fn prune_revoked_delete_uses_revoked_index() {
        use sqlx::Row;
        let p = pool().await;
        let plan: String =
            sqlx::query("EXPLAIN QUERY PLAN DELETE FROM sessions WHERE revoked_at IS NOT NULL")
                .fetch_all(&p)
                .await
                .unwrap()
                .iter()
                .map(|r| r.get::<String, _>("detail"))
                .collect::<Vec<_>>()
                .join("\n");
        assert!(
            plan.contains("idx_sessions_revoked_at"),
            "revoked-prune delete should use idx_sessions_revoked_at, got plan:\n{plan}"
        );
    }
}
