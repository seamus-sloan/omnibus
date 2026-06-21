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
