//! The admin user projection and management: `list_users` ordering, its
//! response ceiling and the display name it surfaces, admin-created users
//! bypassing the registration gate, permission updates that refuse to
//! demote the last admin, and deletion that cascades and protects the
//! last admin.

use super::super::*;
use crate::auth::test_support::pool;

use super::{ADMIN, READER};
use omnibus_shared::UserPermissions;

/// Bulk-insert `count` user rows without going through `create_user` /
/// `admin_create_user` — used only to exercise `list_users`'s
/// `LIST_USERS_LIMIT` in isolation, well above what a real registration
/// flow would ever populate in a test.
async fn seed_users_raw(pool: &SqlitePool, count: i64) {
    sqlx::query(
        r"
        WITH RECURSIVE n(i) AS (
            SELECT 1 UNION ALL SELECT i + 1 FROM n WHERE i < ?
        )
        INSERT INTO users (username, password_hash, created_at)
        SELECT 'bulk-user-' || i, 'not-a-real-hash', i FROM n
        ",
    )
    .bind(count)
    .execute(pool)
    .await
    .unwrap();
}

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

/// `list_users` must not return more than `LIST_USERS_LIMIT` rows even when
/// the underlying table holds more.
#[tokio::test]
async fn list_users_caps_response_at_list_users_limit() {
    let p = pool().await;
    let over_cap = LIST_USERS_LIMIT + 50;
    seed_users_raw(&p, over_cap).await;

    let rows = list_users(&p).await.unwrap();
    assert_eq!(
        rows.len() as i64,
        LIST_USERS_LIMIT,
        "list_users must not return more than LIST_USERS_LIMIT rows",
    );
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

#[tokio::test]
async fn list_users_surfaces_the_display_name_for_the_admin_table() {
    let p = pool().await;
    let alice = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    set_display_name(&p, alice.id, Some("Alice A."))
        .await
        .unwrap();

    let rows = list_users(&p).await.unwrap();

    let row = rows.iter().find(|r| r.id == alice.id).unwrap();
    assert_eq!(row.display_name.as_deref(), Some("Alice A."));
    assert_eq!(row.username, "alice");
}
