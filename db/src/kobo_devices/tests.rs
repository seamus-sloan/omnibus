//! Unit tests for `kobo_devices`: create/list/get, token regenerate (rotates
//! the token, preserves cursor + learned device id), revoke, token resolution
//! (valid / invalid / cross-user), device-id steal-on-learn + hardware-id resolution, and the NotFound
//! variant on regenerate/revoke of an unowned id.

use super::*;
use crate::init_db;

async fn seed_user(pool: &SqlitePool, name: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
         VALUES (?, '!x', 0, 0, 0, 1) RETURNING id",
    )
    .bind(name)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn create_device_mints_a_token_and_lists_it() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;

    let dev = create_device(&pool, user, "Kobo Clara").await.unwrap();
    assert_eq!(dev.user_id, user);
    assert_eq!(dev.name, "Kobo Clara");
    assert!(!dev.token.is_empty());
    assert!(dev.kobo_device_id.is_none());
    assert!(dev.sync_cursor.is_none());

    let listed = list_devices(&pool, user).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, dev.id);
    assert_eq!(listed[0].token, dev.token);
}

#[tokio::test]
async fn create_device_mints_unique_tokens_per_device() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;

    let a = create_device(&pool, user, "Kobo One").await.unwrap();
    let b = create_device(&pool, user, "Kobo Two").await.unwrap();
    assert_ne!(a.token, b.token);
    assert_eq!(list_devices(&pool, user).await.unwrap().len(), 2);
}

#[tokio::test]
async fn resolve_device_by_token_returns_owner_for_valid_token() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let dev = create_device(&pool, user, "Kobo").await.unwrap();

    let resolved = resolve_device_by_token(&pool, &dev.token)
        .await
        .unwrap()
        .expect("valid token resolves");
    assert_eq!(resolved.user_id, user);
    assert_eq!(resolved.device_id, dev.id);
}

#[tokio::test]
async fn resolve_device_by_token_returns_none_for_unknown_token() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    create_device(&pool, user, "Kobo").await.unwrap();

    assert!(resolve_device_by_token(&pool, "not-a-real-token")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn regenerate_token_rotates_the_token_and_invalidates_the_old() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let dev = create_device(&pool, user, "Kobo").await.unwrap();
    let old = dev.token.clone();

    let rotated = regenerate_token(&pool, user, dev.id).await.unwrap();
    assert_ne!(rotated.token, old);
    assert_eq!(rotated.id, dev.id);

    // Old token no longer authenticates; new one does.
    assert!(resolve_device_by_token(&pool, &old)
        .await
        .unwrap()
        .is_none());
    assert!(resolve_device_by_token(&pool, &rotated.token)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn regenerate_token_preserves_the_sync_cursor() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let dev = create_device(&pool, user, "Kobo").await.unwrap();
    set_sync_cursor(&pool, dev.id, "cursor-42").await.unwrap();

    // AC3: the cursor is independent of the auth token — rotating the token
    // must not disturb delta state.
    let rotated = regenerate_token(&pool, user, dev.id).await.unwrap();
    assert_eq!(rotated.sync_cursor.as_deref(), Some("cursor-42"));
}

#[tokio::test]
async fn regenerate_token_rejects_a_device_owned_by_another_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = seed_user(&pool, "owner").await;
    let other = seed_user(&pool, "other").await;
    let dev = create_device(&pool, owner, "Kobo").await.unwrap();

    let err = regenerate_token(&pool, other, dev.id).await.unwrap_err();
    assert!(matches!(err, KoboDeviceError::NotFound));
    // The owner's token is untouched.
    assert!(resolve_device_by_token(&pool, &dev.token)
        .await
        .unwrap()
        .is_some());
}

#[tokio::test]
async fn revoke_device_removes_the_token() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let dev = create_device(&pool, user, "Kobo").await.unwrap();

    revoke_device(&pool, user, dev.id).await.unwrap();
    assert!(list_devices(&pool, user).await.unwrap().is_empty());
    assert!(resolve_device_by_token(&pool, &dev.token)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn revoke_device_rejects_a_device_owned_by_another_user() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let owner = seed_user(&pool, "owner").await;
    let other = seed_user(&pool, "other").await;
    let dev = create_device(&pool, owner, "Kobo").await.unwrap();

    let err = revoke_device(&pool, other, dev.id).await.unwrap_err();
    assert!(matches!(err, KoboDeviceError::NotFound));
    assert_eq!(list_devices(&pool, owner).await.unwrap().len(), 1);
}

#[tokio::test]
async fn list_devices_is_scoped_to_the_owner() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    create_device(&pool, alice, "Alice Kobo").await.unwrap();
    create_device(&pool, bob, "Bob Kobo").await.unwrap();

    let alice_devices = list_devices(&pool, alice).await.unwrap();
    assert_eq!(alice_devices.len(), 1);
    assert_eq!(alice_devices[0].name, "Alice Kobo");
    // get_device_for_user won't cross the ownership boundary (AC4).
    assert!(get_device_for_user(&pool, alice, alice_devices[0].id)
        .await
        .unwrap()
        .is_some());
    assert!(get_device_for_user(&pool, bob, alice_devices[0].id)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn learn_kobo_device_id_rebinds_a_new_id_to_the_same_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let dev = create_device(&pool, user, "Kobo").await.unwrap();

    learn_kobo_device_id(&pool, dev.id, "hw-abc").await.unwrap();
    let after = get_device_for_user(&pool, user, dev.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(after.kobo_device_id.as_deref(), Some("hw-abc"));

    // A factory-reset device reports a fresh hardware id on its next tokened
    // call; the row follows the token, so the id re-binds in place.
    learn_kobo_device_id(&pool, dev.id, "hw-xyz").await.unwrap();
    let again = get_device_for_user(&pool, user, dev.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(again.kobo_device_id.as_deref(), Some("hw-xyz"));
}

#[tokio::test]
async fn learn_kobo_device_id_steals_the_id_from_another_users_row() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let alice = seed_user(&pool, "alice").await;
    let bob = seed_user(&pool, "bob").await;
    let alices = create_device(&pool, alice, "Kobo").await.unwrap();
    let bobs = create_device(&pool, bob, "Kobo").await.unwrap();

    // The physical device synced under Alice's token first, then was handed
    // to Bob who registered his own. Presenting Bob's token moves the
    // hardware id — Alice's row must not retain it (unique index, 0060).
    learn_kobo_device_id(&pool, alices.id, "hw-shared")
        .await
        .unwrap();
    learn_kobo_device_id(&pool, bobs.id, "hw-shared")
        .await
        .unwrap();

    let alices_after = get_device_for_user(&pool, alice, alices.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(alices_after.kobo_device_id, None);
    let resolved = resolve_device_by_hardware_id(&pool, "hw-shared")
        .await
        .unwrap()
        .expect("bob's row must resolve");
    assert_eq!(resolved.device_id, bobs.id);
    assert_eq!(resolved.user_id, bob);
}

#[tokio::test]
async fn resolve_device_by_hardware_id_returns_the_owning_device_and_touches_last_seen() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let dev = create_device(&pool, user, "Kobo").await.unwrap();
    learn_kobo_device_id(&pool, dev.id, "hw-abc").await.unwrap();

    sqlx::query("UPDATE kobo_devices SET last_seen_at = 1 WHERE id = ?")
        .bind(dev.id)
        .execute(&pool)
        .await
        .unwrap();

    let resolved = resolve_device_by_hardware_id(&pool, "hw-abc")
        .await
        .unwrap()
        .expect("learned id must resolve");
    assert_eq!(resolved.device_id, dev.id);
    assert_eq!(resolved.user_id, user);

    let after = get_device_for_user(&pool, user, dev.id)
        .await
        .unwrap()
        .unwrap();
    assert!(after.last_seen_at > 1, "resolve must touch last_seen_at");
}

#[tokio::test]
async fn resolve_device_by_hardware_id_returns_none_for_an_unknown_id() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    create_device(&pool, user, "Kobo").await.unwrap();

    assert!(resolve_device_by_hardware_id(&pool, "hw-never-learned")
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn resolve_device_by_hardware_id_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    pool.close().await;
    let err = resolve_device_by_hardware_id(&pool, "hw-abc").await;
    assert!(matches!(err, Err(KoboDeviceError::Sqlx(_))));
}

#[tokio::test]
async fn deleting_the_user_cascades_the_device() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool, "reader").await;
    let dev = create_device(&pool, user, "Kobo").await.unwrap();

    sqlx::query("DELETE FROM users WHERE id = ?")
        .bind(user)
        .execute(&pool)
        .await
        .unwrap();
    assert!(resolve_device_by_token(&pool, &dev.token)
        .await
        .unwrap()
        .is_none());
}
