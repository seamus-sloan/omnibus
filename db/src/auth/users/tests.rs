use super::*;
use crate::auth::test_support::pool;

#[tokio::test]
async fn first_user_is_admin_and_disables_registration() {
    let p = pool().await;
    assert!(registration_enabled(&p).await.unwrap());
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    assert!(u.is_admin);
    assert!(u.can_upload);
    assert!(u.can_edit);
    assert!(u.can_download);
    assert!(!registration_enabled(&p).await.unwrap());
}

#[tokio::test]
async fn second_user_needs_registration_enabled() {
    let p = pool().await;
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    // Registration auto-disabled after first user.
    let err = create_user(&p, "bob", "bunker9-longer-pass")
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::RegistrationDisabled));
    // Admin re-enables.
    set_registration_enabled(&p, true).await.unwrap();
    let bob = create_user(&p, "bob", "bunker9-longer-pass").await.unwrap();
    assert!(!bob.is_admin);
    assert!(!bob.can_upload);
    assert!(!bob.can_edit);
    assert!(bob.can_download);
}

#[tokio::test]
async fn username_collision_nocase() {
    let p = pool().await;
    create_user(&p, "Alice", "hunter2-real-long").await.unwrap();
    set_registration_enabled(&p, true).await.unwrap();
    let err = create_user(&p, "alice", "hunter2-real-long")
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::UsernameTaken));
}

#[tokio::test]
async fn concurrent_first_user_race_only_one_admin() {
    // BEGIN IMMEDIATE serializes the two registrations on the RESERVED
    // write lock: whichever transaction wins inserts the first user as
    // admin and flips registration_enabled to '0' before committing. The
    // loser blocks until that commit, then sees user_count = 1 with
    // registration disabled and is rejected. So the race resolves
    // deterministically — exactly one user, who is the admin, and exactly
    // one Ok. This is the race we are specifically defending against.
    let p = pool().await;

    let p1 = p.clone();
    let p2 = p.clone();
    let t1 = tokio::spawn(async move { create_user(&p1, "alice", "hunter2-real-long").await });
    let t2 = tokio::spawn(async move { create_user(&p2, "bob", "bunker9-longer-pass").await });

    let r1 = t1.await.unwrap();
    let r2 = t2.await.unwrap();

    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&p)
        .await
        .unwrap();
    let admins: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users WHERE is_admin = 1")
        .fetch_one(&p)
        .await
        .unwrap();

    // The loser is rejected before inserting, so exactly one user exists
    // and it's the admin; exactly one create returned Ok.
    assert_eq!(users, 1, "loser never inserts — exactly one user survives");
    assert_eq!(admins, 1, "exactly one admin regardless of race outcome");
    assert!(
        r1.is_ok() ^ r2.is_ok(),
        "exactly one create succeeds; the other is rejected"
    );
}

#[tokio::test]
async fn create_user_error_path_rolls_back_cleanly() {
    // A `?` early-return between BEGIN IMMEDIATE and COMMIT (here a
    // UsernameTaken reject) must drop the transaction without a partial
    // commit: no extra user row, `registration_enabled` untouched, and a
    // subsequent valid create still succeeds on the same connection pool.
    let p = pool().await;
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    set_registration_enabled(&p, true).await.unwrap();

    // Collide on the existing username — returns inside the transaction.
    let err = create_user(&p, "alice", "different-long-pass")
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::UsernameTaken));

    // The failed attempt left no trace: still exactly one user, and the
    // admin's registration toggle wasn't flipped by the rolled-back tx.
    let users: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM users")
        .fetch_one(&p)
        .await
        .unwrap();
    assert_eq!(users, 1, "rolled-back create must not insert a row");
    assert!(
        registration_enabled(&p).await.unwrap(),
        "registration toggle must survive the rollback"
    );

    // The connection is back in a usable, non-stuck state.
    let bob = create_user(&p, "bob", "bunker9-longer-pass").await.unwrap();
    assert!(!bob.is_admin);
}

#[tokio::test]
async fn create_user_rejects_invalid_username() {
    // create_user runs validate_username before touching the DB, so an
    // empty/control-char/oversize username should error out without
    // consuming the first-user-admin slot.
    let p = pool().await;

    let cases: &[(&str, &str)] = &[
        ("", "username must not be empty"),
        ("ali\0ce", "invalid control character"),
        (" alice", "leading or trailing whitespace"),
    ];
    for (input, needle) in cases {
        let err = create_user(&p, input, "hunter2-real-long")
            .await
            .unwrap_err();
        assert!(
            matches!(&err, AuthError::Validation(m) if m.contains(needle)),
            "input {input:?}: expected Validation containing {needle:?}, got {err:?}",
        );
    }

    // None of the rejected attempts created a row, so the first valid
    // create still gets the admin slot.
    let alice = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    assert!(alice.is_admin);
}

#[tokio::test]
async fn promote_to_admin_idempotent() {
    let p = pool().await;
    set_registration_enabled(&p, true).await.unwrap();
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    set_registration_enabled(&p, true).await.unwrap();
    create_user(&p, "bob", "bunker9-longer-pass").await.unwrap();
    assert!(promote_to_admin(&p, "bob").await.unwrap());
    let bob = get_user_by_username(&p, "bob").await.unwrap().unwrap();
    assert!(bob.is_admin);
    // No-op on unknown user.
    assert!(!promote_to_admin(&p, "eve").await.unwrap());
}

#[tokio::test]
async fn set_kindle_email_roundtrips_and_clears() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    assert_eq!(u.kindle_email, None);

    set_kindle_email(&p, u.id, Some("alice@kindle.com"))
        .await
        .unwrap();
    assert_eq!(
        get_kindle_email(&p, u.id).await.unwrap().as_deref(),
        Some("alice@kindle.com")
    );
    let reloaded = get_user_by_id(&p, u.id).await.unwrap().unwrap();
    assert_eq!(reloaded.kindle_email.as_deref(), Some("alice@kindle.com"));

    // Clearing with None wipes it.
    set_kindle_email(&p, u.id, None).await.unwrap();
    assert_eq!(get_kindle_email(&p, u.id).await.unwrap(), None);
}

#[tokio::test]
async fn set_kindle_email_rejects_malformed_address() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let err = set_kindle_email(&p, u.id, Some("nope")).await.unwrap_err();
    assert!(matches!(err, AuthError::Validation(_)));
}

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

    let callers_session = create_session(&p, u.id, None, SessionKind::Cookie, 3600)
        .await
        .unwrap();
    let other_session = create_session(&p, u.id, None, SessionKind::Bearer, 3600)
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

// ── Admin user management (F5.4) ─────────────────────────────────────

use omnibus_shared::UserPermissions;

const READER: UserPermissions = UserPermissions {
    is_admin: false,
    can_upload: false,
    can_edit: false,
    can_download: true,
};
const ADMIN: UserPermissions = UserPermissions {
    is_admin: true,
    can_upload: true,
    can_edit: true,
    can_download: true,
};

#[tokio::test]
async fn list_users_returns_projection_ordered_oldest_first() {
    let p = pool().await;
    let admin = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let reader = admin_create_user(&p, "bob", "bunker9-longer-pass", READER)
        .await
        .unwrap();

    let rows = list_users(&p).await.unwrap();
    assert_eq!(rows.len(), 2);
    // Oldest first: alice (first user, admin) then bob.
    assert_eq!(rows[0].id, admin.id);
    assert!(rows[0].is_admin && !rows[0].locked);
    assert_eq!(rows[1].id, reader.id);
    assert!(!rows[1].is_admin);
    assert!(rows[1].can_download && !rows[1].can_upload);
}

#[tokio::test]
async fn admin_create_user_sets_permissions_and_bypasses_registration_gate() {
    let p = pool().await;
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    // Registration auto-disabled after first user — admin_create ignores it.
    assert!(!registration_enabled(&p).await.unwrap());

    let row = admin_create_user(&p, "bob", "bunker9-longer-pass", ADMIN)
        .await
        .unwrap();
    assert!(row.is_admin && row.can_upload && row.can_edit && row.can_download);
    assert!(!row.locked && row.kindle_email.is_none());

    // The new user can actually authenticate.
    crate::auth::verify_login(&p, "bob", "bunker9-longer-pass")
        .await
        .unwrap();
}

#[tokio::test]
async fn admin_create_user_rejects_duplicate_username_nocase() {
    let p = pool().await;
    create_user(&p, "Alice", "hunter2-real-long").await.unwrap();
    let err = admin_create_user(&p, "alice", "bunker9-longer-pass", READER)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::UsernameTaken));
}

#[tokio::test]
async fn admin_create_user_rejects_invalid_username_and_password() {
    let p = pool().await;
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    let err = admin_create_user(&p, " bob", "bunker9-longer-pass", READER)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::Validation(_)));

    let err = admin_create_user(&p, "bob", "short", READER)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::Validation(_)));
}

#[tokio::test]
async fn update_user_permissions_replaces_flags() {
    let p = pool().await;
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let bob = admin_create_user(&p, "bob", "bunker9-longer-pass", READER)
        .await
        .unwrap();

    update_user_permissions(
        &p,
        bob.id,
        UserPermissions {
            is_admin: false,
            can_upload: true,
            can_edit: true,
            can_download: false,
        },
    )
    .await
    .unwrap();

    let bob_row = list_users(&p)
        .await
        .unwrap()
        .into_iter()
        .find(|u| u.id == bob.id)
        .unwrap();
    assert!(bob_row.can_upload && bob_row.can_edit);
    assert!(!bob_row.can_download && !bob_row.is_admin);
}

#[tokio::test]
async fn update_user_permissions_refuses_to_demote_last_admin() {
    let p = pool().await;
    let alice = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    // Alice is the only admin — demoting her is refused.
    let err = update_user_permissions(&p, alice.id, READER)
        .await
        .unwrap_err();
    assert!(matches!(err, AuthError::LastAdmin));

    // Promote a second admin, then demoting alice is allowed.
    let bob = admin_create_user(&p, "bob", "bunker9-longer-pass", ADMIN)
        .await
        .unwrap();
    assert!(bob.is_admin);
    update_user_permissions(&p, alice.id, READER).await.unwrap();
}

#[tokio::test]
async fn update_user_permissions_rejects_unknown_user() {
    let p = pool().await;
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let err = update_user_permissions(&p, 9999, READER).await.unwrap_err();
    assert!(matches!(err, AuthError::UserNotFound));
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

    let bobs_cookie = create_session(&p, bob.id, None, SessionKind::Cookie, 3600)
        .await
        .unwrap();
    let bobs_bearer = create_session(&p, bob.id, None, SessionKind::Bearer, 3600)
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

#[tokio::test]
async fn delete_user_removes_row_and_cascades() {
    let p = pool().await;
    create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let bob = admin_create_user(&p, "bob", "bunker9-longer-pass", READER)
        .await
        .unwrap();
    // Bob got a Wishlist shelf on create — it must cascade away with him.
    let shelves_before: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM shelves WHERE owner_user_id = ?")
            .bind(bob.id)
            .fetch_one(&p)
            .await
            .unwrap();
    assert!(shelves_before >= 1);

    delete_user(&p, bob.id).await.unwrap();

    assert!(get_user_by_id(&p, bob.id).await.unwrap().is_none());
    let shelves_after: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM shelves WHERE owner_user_id = ?")
            .bind(bob.id)
            .fetch_one(&p)
            .await
            .unwrap();
    assert_eq!(shelves_after, 0, "owned shelves must cascade-delete");
}

#[tokio::test]
async fn delete_user_refuses_to_delete_last_admin() {
    let p = pool().await;
    let alice = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    let err = delete_user(&p, alice.id).await.unwrap_err();
    assert!(matches!(err, AuthError::LastAdmin));

    // With a second admin present, deleting alice is allowed.
    admin_create_user(&p, "bob", "bunker9-longer-pass", ADMIN)
        .await
        .unwrap();
    delete_user(&p, alice.id).await.unwrap();
    assert!(get_user_by_id(&p, alice.id).await.unwrap().is_none());
}

#[tokio::test]
async fn delete_user_rejects_unknown_user() {
    let p = pool().await;
    let err = delete_user(&p, 9999).await.unwrap_err();
    assert!(matches!(err, AuthError::UserNotFound));
}
