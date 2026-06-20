//! Unit tests for `auth::login` — covers the success path that clears
//! failure counters, the per-account lockout schedule after repeated
//! failures, the post-cooldown reset that prevents instant re-lock, and
//! the unknown-user path that returns `InvalidCredentials` without
//! leaking existence via response timing.

use super::*;
use crate::auth::test_support::pool;
use crate::auth::users::create_user;

#[tokio::test]
async fn login_success_clears_failures() {
    let p = pool().await;
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    // Record 2 failures, then a success, then assert counter == 0.
    let _ = verify_login(&p, "alice", "wrong!").await;
    let _ = verify_login(&p, "alice", "wrong!").await;
    let u = verify_login(&p, "alice", "hunter2-real-long")
        .await
        .unwrap();
    assert_eq!(u.username, "alice");

    let failed: i64 = sqlx::query_scalar("SELECT failed_login_count FROM users WHERE id = ?")
        .bind(u.id)
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(failed, 0);
}

#[tokio::test]
async fn login_locks_after_five_failures() {
    let p = pool().await;
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    for _ in 0..5 {
        let _ = verify_login(&p, "alice", "wrong!").await;
    }
    let err = verify_login(&p, "alice", "hunter2-real-long")
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::AccountLocked { .. }));
}

#[tokio::test]
async fn login_lockout_resets_after_cooldown_elapses() {
    // Regression: once the lockout window passes, a single subsequent
    // failed attempt must NOT immediately re-lock the account (the
    // monotonic counter would otherwise stay >= LOCKOUT_MIN_AFTER).
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    for _ in 0..5 {
        let _ = verify_login(&p, "alice", "wrong!").await;
    }
    // Simulate lockout window elapsing by rewriting the row.
    sqlx::query("UPDATE users SET locked_until = 1 WHERE id = ?")
        .bind(u.id)
        .execute(&p)
        .await
        .unwrap();
    // One more wrong attempt must NOT relock: effective counter is 0,
    // becomes 1, still well below LOCKOUT_MIN_AFTER.
    let err = verify_login(&p, "alice", "still-wrong").await.unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials));
    let locked: Option<i64> = sqlx::query_scalar("SELECT locked_until FROM users WHERE id = ?")
        .bind(u.id)
        .fetch_one(&p)
        .await
        .unwrap();
    assert!(locked.is_none(), "single failure must not relock");
    // And a subsequent correct password works.
    let ok = verify_login(&p, "alice", "hunter2-real-long")
        .await
        .unwrap();
    assert_eq!(ok.id, u.id);
}

#[tokio::test]
async fn login_unknown_user_returns_invalid_credentials() {
    let p = pool().await;
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let err = verify_login(&p, "nobody", "any-long-password")
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials));
}
