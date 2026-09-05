//! `set_display_name`: stored and denormalized into the owner's wishlist
//! shelf name, cleared (or blanked) back to the username, trimmed, capped,
//! control characters rejected, and only the named user's shelf touched.

use super::super::*;
use crate::auth::test_support::pool;

use super::ADMIN;

/// The Wishlist shelf name for a user, which denormalizes
/// `COALESCE(display_name, username) || "'s Wishlist"`.
async fn wishlist_name(p: &sqlx::SqlitePool, user_id: i64) -> String {
    sqlx::query_scalar("SELECT name FROM shelves WHERE owner_user_id = ? AND kind = 'wishlist'")
        .bind(user_id)
        .fetch_one(p)
        .await
        .unwrap()
}

#[tokio::test]
async fn set_display_name_stores_the_name_and_renames_the_wishlist_shelf() {
    let p = pool().await;
    let u = create_user(&p, "cool-guy-7", "hunter2-real-long")
        .await
        .unwrap();
    assert_eq!(wishlist_name(&p, u.id).await, "cool-guy-7's Wishlist");

    set_display_name(&p, u.id, Some("Seamus")).await.unwrap();

    let stored = get_user_by_id(&p, u.id).await.unwrap().unwrap();
    assert_eq!(stored.display_name.as_deref(), Some("Seamus"));
    assert_eq!(stored.username, "cool-guy-7", "username is not renamed");
    assert_eq!(wishlist_name(&p, u.id).await, "Seamus's Wishlist");
}

#[tokio::test]
async fn set_display_name_clearing_reverts_to_the_username_everywhere() {
    let p = pool().await;
    let u = create_user(&p, "cool-guy-7", "hunter2-real-long")
        .await
        .unwrap();
    set_display_name(&p, u.id, Some("Seamus")).await.unwrap();

    set_display_name(&p, u.id, None).await.unwrap();

    assert!(get_user_by_id(&p, u.id)
        .await
        .unwrap()
        .unwrap()
        .display_name
        .is_none());
    assert_eq!(wishlist_name(&p, u.id).await, "cool-guy-7's Wishlist");
}

#[tokio::test]
async fn set_display_name_treats_a_blank_name_as_a_clear() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    set_display_name(&p, u.id, Some("Alice")).await.unwrap();

    set_display_name(&p, u.id, Some("   ")).await.unwrap();

    assert!(get_user_by_id(&p, u.id)
        .await
        .unwrap()
        .unwrap()
        .display_name
        .is_none());
    assert_eq!(wishlist_name(&p, u.id).await, "alice's Wishlist");
}

#[tokio::test]
async fn set_display_name_trims_surrounding_whitespace() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    set_display_name(&p, u.id, Some("  Seamus  "))
        .await
        .unwrap();

    let stored = get_user_by_id(&p, u.id).await.unwrap().unwrap();
    assert_eq!(stored.display_name.as_deref(), Some("Seamus"));
    assert_eq!(wishlist_name(&p, u.id).await, "Seamus's Wishlist");
}

#[tokio::test]
async fn set_display_name_rejects_a_name_over_the_length_cap() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    let too_long = "a".repeat(DISPLAY_NAME_MAX_LEN + 1);
    let err = set_display_name(&p, u.id, Some(&too_long))
        .await
        .unwrap_err();

    assert!(matches!(err, AuthError::Validation(_)));
    // A rejected write leaves both the row and the shelf label untouched.
    assert!(get_user_by_id(&p, u.id)
        .await
        .unwrap()
        .unwrap()
        .display_name
        .is_none());
    assert_eq!(wishlist_name(&p, u.id).await, "alice's Wishlist");
}

#[tokio::test]
async fn set_display_name_accepts_a_name_exactly_at_the_length_cap() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    let exact = "a".repeat(DISPLAY_NAME_MAX_LEN);
    set_display_name(&p, u.id, Some(&exact)).await.unwrap();

    assert_eq!(
        get_user_by_id(&p, u.id)
            .await
            .unwrap()
            .unwrap()
            .display_name,
        Some(exact)
    );
}

#[tokio::test]
async fn set_display_name_rejects_control_characters() {
    let p = pool().await;
    let u = create_user(&p, "alice", "hunter2-real-long").await.unwrap();

    let err = set_display_name(&p, u.id, Some("Sea\nmus"))
        .await
        .unwrap_err();

    assert!(matches!(err, AuthError::Validation(_)));
}

#[tokio::test]
async fn set_display_name_only_touches_the_named_users_wishlist() {
    let p = pool().await;
    let alice = create_user(&p, "alice", "hunter2-real-long").await.unwrap();
    let bob = admin_create_user(&p, "bob", "bunker9-longer-pass", ADMIN)
        .await
        .unwrap();

    set_display_name(&p, alice.id, Some("Alice A."))
        .await
        .unwrap();

    assert_eq!(wishlist_name(&p, alice.id).await, "Alice A.'s Wishlist");
    assert_eq!(wishlist_name(&p, bob.id).await, "bob's Wishlist");
}
