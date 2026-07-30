//! Tests for session lifecycle: create/lookup round-tripping, expiry and
//! idle-timeout enforcement, revocation (single, all, all-except-current),
//! last-used touch throttling, and cookie/bearer token resolution.

use super::*;
use crate::auth::test_support::pool;
use crate::auth::token::SESSION_COOKIE_NAME;
use crate::auth::users::create_user;

#[tokio::test]
async fn create_session_and_lookup_session_round_trips_a_cookie_session() {
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
async fn revoke_all_sessions_for_user_revokes_every_active_session() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let a = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
        .await
        .unwrap();
    let b = create_session(&p, u.id, None, SessionKind::Bearer, 3600)
        .await
        .unwrap();

    let revoked = revoke_all_sessions_for_user(&p, u.id).await.unwrap();
    assert_eq!(revoked, 2);

    assert!(matches!(
        lookup_session(&p, &a.raw_token).await.unwrap_err(),
        AuthError::SessionNotFound
    ));
    assert!(matches!(
        lookup_session(&p, &b.raw_token).await.unwrap_err(),
        AuthError::SessionNotFound
    ));
}

#[tokio::test]
async fn revoke_all_sessions_for_user_is_scoped_to_the_target_user() {
    let p = pool().await;
    let alice = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    // Registration auto-disables after the first user; re-enable for bob.
    crate::auth::set_registration_enabled(&p, true)
        .await
        .unwrap();
    let bob = create_user(&p, "bob", "bunker9-longer-pass").await.unwrap();
    let alices = create_session(&p, alice.id, None, SessionKind::Cookie, 3600)
        .await
        .unwrap();
    let bobs = create_session(&p, bob.id, None, SessionKind::Cookie, 3600)
        .await
        .unwrap();

    let revoked = revoke_all_sessions_for_user(&p, alice.id).await.unwrap();
    assert_eq!(revoked, 1);

    assert!(matches!(
        lookup_session(&p, &alices.raw_token).await.unwrap_err(),
        AuthError::SessionNotFound
    ));
    lookup_session(&p, &bobs.raw_token)
        .await
        .expect("bob's session must be untouched by alice's revocation");
}

#[tokio::test]
async fn revoke_all_sessions_for_user_except_preserves_only_the_named_session() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let keep = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
        .await
        .unwrap();
    let drop_a = create_session(&p, u.id, None, SessionKind::Bearer, 3600)
        .await
        .unwrap();
    let drop_b = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
        .await
        .unwrap();

    let revoked = revoke_all_sessions_for_user_except(&p, u.id, keep.session.id)
        .await
        .unwrap();
    assert_eq!(revoked, 2);

    lookup_session(&p, &keep.raw_token)
        .await
        .expect("the excluded session must remain live");
    assert!(matches!(
        lookup_session(&p, &drop_a.raw_token).await.unwrap_err(),
        AuthError::SessionNotFound
    ));
    assert!(matches!(
        lookup_session(&p, &drop_b.raw_token).await.unwrap_err(),
        AuthError::SessionNotFound
    ));
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

/// The idle DELETE must use `idx_sessions_last_used_at`, not scan.
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

/// The revoked DELETE must use the partial `idx_sessions_revoked_at`,
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
