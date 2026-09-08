//! Password changes and lockouts: `change_password` verifying the
//! current one, validating the new one and revoking every other session,
//! the admin reset revoking all of the target's sessions, and
//! `unlock_user`.

use super::super::*;
use crate::auth::test_support::pool;

use super::READER;

#[tokio::test]
async fn change_password_updates_hash_and_stamp_and_new_password_logs_in() {
    use crate::auth::verify_login;

    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let before: i64 = sqlx::query_scalar("SELECT password_changed_at FROM users WHERE id = ?")
        .bind(u.id)
        .fetch_one(&p)
        .await
        .unwrap();

    change_password(&p, u.id, "hunter2-real-long", "brand-new-longpass", -1)
        .await
        .unwrap();

    // Old password no longer authenticates; the new one does.
    assert!(matches!(
        verify_login(&p, "alice", "hunter2-real-long")
            .await
            .unwrap_err(),
        AuthError::InvalidCredentials
    ));
    verify_login(&p, "alice", "brand-new-longpass")
        .await
        .unwrap();

    // The stamp advanced (or held steady within the same wall-clock second);
    // it must never regress.
    let after: i64 = sqlx::query_scalar("SELECT password_changed_at FROM users WHERE id = ?")
        .bind(u.id)
        .fetch_one(&p)
        .await
        .unwrap();
    assert!(after >= before, "password_changed_at must not regress");
}

#[tokio::test]
async fn change_password_rejects_wrong_current_and_leaves_hash_intact() {
    use crate::auth::verify_login;

    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    let err = change_password(&p, u.id, "wrong-current-pass", "brand-new-longpass", -1)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials));

    // Nothing changed: the original password still logs in.
    verify_login(&p, "alice", "hunter2-real-long")
        .await
        .unwrap();
}

#[tokio::test]
async fn change_password_rejects_invalid_new_password() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    // Too short.
    let err = change_password(&p, u.id, "hunter2-real-long", "short", -1)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::Validation(_)));

    // Common-password reject-list.
    let err = change_password(&p, u.id, "hunter2-real-long", "password123", -1)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::Validation(_)));
}

#[tokio::test]
async fn change_password_rejects_unknown_user() {
    let p = pool().await;
    let err = change_password(&p, 9999, "anything-here-long", "brand-new-longpass", -1)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::InvalidCredentials));
}

#[tokio::test]
async fn change_password_keeps_callers_own_session_but_revokes_others() {
    // #1402: a self-service password change must not immediately log the
    // caller out of the session they used to make the change, but any other
    // live session for the account (e.g. a stolen cookie) must die with it.
    use crate::auth::{create_session, lookup_session, SessionKind};

    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    let callers_session = create_session(&p, u.id, None, SessionKind::Cookie, 3600, None)
        .await
        .unwrap();
    let other_session = create_session(&p, u.id, None, SessionKind::Bearer, 3600, None)
        .await
        .unwrap();

    change_password(
        &p,
        u.id,
        "hunter2-real-long",
        "brand-new-longpass",
        callers_session.session.id,
    )
    .await
    .unwrap();

    lookup_session(&p, &callers_session.raw_token)
        .await
        .expect("the caller's own session must survive the password change");

    let err = lookup_session(&p, &other_session.raw_token)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::SessionNotFound));
}

#[tokio::test]
async fn admin_set_password_resets_and_new_password_logs_in() {
    let p = pool().await;
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let bob = admin_create_user(&p, "bob", "bunker9-longer-pass", READER)
        .await
        .unwrap();

    admin_set_password(&p, bob.id, "reset-by-admin-99")
        .await
        .unwrap();

    assert!(matches!(
        crate::auth::verify_login(&p, "bob", "bunker9-longer-pass")
            .await
            .unwrap_err(),
        AuthError::InvalidCredentials
    ));
    crate::auth::verify_login(&p, "bob", "reset-by-admin-99")
        .await
        .unwrap();
}

#[tokio::test]
async fn admin_set_password_revokes_all_sessions_for_target_user() {
    // #1402: an admin-initiated reset has no caller session on the target
    // account to preserve — every existing session for that user must die.
    use crate::auth::{create_session, lookup_session, SessionKind};

    let p = pool().await;
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let bob = admin_create_user(&p, "bob", "bunker9-longer-pass", READER)
        .await
        .unwrap();

    let bobs_cookie = create_session(&p, bob.id, None, SessionKind::Cookie, 3600, None)
        .await
        .unwrap();
    let bobs_bearer = create_session(&p, bob.id, None, SessionKind::Bearer, 3600, None)
        .await
        .unwrap();

    admin_set_password(&p, bob.id, "reset-by-admin-99")
        .await
        .unwrap();

    let err = lookup_session(&p, &bobs_cookie.raw_token)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::SessionNotFound));
    let err = lookup_session(&p, &bobs_bearer.raw_token)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::SessionNotFound));
}

#[tokio::test]
async fn admin_set_password_rejects_invalid_and_unknown() {
    let p = pool().await;
    let alice = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    let err = admin_set_password(&p, alice.id, "short").await.unwrap_err();
    assert!(matches!(err, AuthError::Validation(_)));

    let err = admin_set_password(&p, 9999, "valid-long-pass-1")
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::UserNotFound));
}

#[tokio::test]
async fn unlock_user_clears_lockout() {
    use crate::auth::verify_login;

    let p = pool().await;
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let bob = admin_create_user(&p, "bob", "bunker9-longer-pass", READER)
        .await
        .unwrap();

    // Force a live lockout far into the future.
    sqlx::query("UPDATE users SET failed_login_count = 5, locked_until = strftime('%s','now') + 3600 WHERE id = ?")
        .bind(bob.id)
        .execute(&p)
        .await
        .unwrap();
    assert!(
        list_users(&p)
            .await
            .unwrap()
            .into_iter()
            .find(|u| u.id == bob.id)
            .unwrap()
            .locked
    );

    unlock_user(&p, bob.id).await.unwrap();

    let row = list_users(&p)
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.id == bob.id)
        .unwrap();
    assert!(!row.locked);
    // And login is no longer blocked by the lockout.
    verify_login(&p, "bob", "bunker9-longer-pass")
        .await
        .unwrap();
}

#[tokio::test]
async fn unlock_user_rejects_unknown_user() {
    let p = pool().await;
    let err = unlock_user(&p, 9999).await.unwrap_err();
    assert!(matches!(err, AuthError::UserNotFound));
}
