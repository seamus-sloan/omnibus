use super::{shelf_for_edit, shelf_for_view, AuthUser};
use omnibus_shared::{CreateShelfRequest, ShelfKind, UserPermissions, Visibility};

/// Two distinct users so ownership/admin gates have someone to deny.
/// The second user goes through `admin_create_user` — plain `create_user`
/// (self-registration) refuses a second account unless
/// `registration_enabled` is set, which these tests have no reason to
/// touch.
async fn pool_with_two_users() -> (sqlx::SqlitePool, i64, i64) {
    let pool = omnibus_db::init_db("sqlite::memory:").await.unwrap();
    // First registered user is auto-admin (see `create_user`).
    let owner_id = omnibus_db::auth::create_user(&pool, "owner", "securepassword1")
        .await
        .unwrap()
        .id;
    let other_id = omnibus_db::auth::admin_create_user(
        &pool,
        "other",
        "securepassword1",
        UserPermissions {
            is_admin: false,
            can_upload: false,
            can_edit: false,
            can_download: true,
        },
    )
    .await
    .unwrap()
    .id;
    (pool, owner_id, other_id)
}

fn auth_user(id: i64, is_admin: bool) -> AuthUser {
    AuthUser {
        id,
        is_admin,
        can_edit: is_admin,
        session_id: 0,
    }
}

async fn seed_shelf(pool: &sqlx::SqlitePool, owner_id: i64, visibility: Visibility) -> i64 {
    let req = CreateShelfRequest {
        kind: ShelfKind::Manual,
        name: "My Shelf".into(),
        description: None,
        visibility,
        match_mode: None,
        rules: vec![],
        book_uuids: vec![],
    };
    omnibus_db::create_shelf(pool, owner_id, &req)
        .await
        .unwrap()
        .id
}

#[tokio::test]
async fn shelf_for_view_allows_the_owner() {
    let (pool, owner_id, _other_id) = pool_with_two_users().await;
    let id = seed_shelf(&pool, owner_id, Visibility::Private).await;

    let shelf = shelf_for_view(&pool, id, &auth_user(owner_id, false))
        .await
        .unwrap();
    assert_eq!(shelf.id, id);
}

#[tokio::test]
async fn shelf_for_view_allows_an_admin_who_is_not_the_owner() {
    let (pool, owner_id, other_id) = pool_with_two_users().await;
    let id = seed_shelf(&pool, owner_id, Visibility::Private).await;

    let shelf = shelf_for_view(&pool, id, &auth_user(other_id, true))
        .await
        .unwrap();
    assert_eq!(shelf.id, id);
}

#[tokio::test]
async fn shelf_for_view_allows_anyone_when_the_shelf_is_public() {
    let (pool, owner_id, other_id) = pool_with_two_users().await;
    let id = seed_shelf(&pool, owner_id, Visibility::Public).await;

    let shelf = shelf_for_view(&pool, id, &auth_user(other_id, false))
        .await
        .unwrap();
    assert_eq!(shelf.id, id);
}

#[tokio::test]
async fn shelf_for_view_hides_a_private_shelf_from_a_non_owner_non_admin() {
    let (pool, owner_id, other_id) = pool_with_two_users().await;
    let id = seed_shelf(&pool, owner_id, Visibility::Private).await;

    let err = shelf_for_view(&pool, id, &auth_user(other_id, false))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("shelf not found"));
}

#[tokio::test]
async fn shelf_for_view_reports_an_unknown_id_identically_to_a_hidden_shelf() {
    let (pool, owner_id, other_id) = pool_with_two_users().await;
    let hidden_id = seed_shelf(&pool, owner_id, Visibility::Private).await;

    let hidden_err = shelf_for_view(&pool, hidden_id, &auth_user(other_id, false))
        .await
        .unwrap_err();
    let missing_err = shelf_for_view(&pool, hidden_id + 999, &auth_user(other_id, false))
        .await
        .unwrap_err();

    // Existence must not be leaked: both cases produce the exact same
    // client-facing message.
    assert_eq!(hidden_err.to_string(), missing_err.to_string());
}

#[tokio::test]
async fn shelf_for_edit_allows_the_owner() {
    let (pool, owner_id, _other_id) = pool_with_two_users().await;
    let id = seed_shelf(&pool, owner_id, Visibility::Private).await;

    let shelf = shelf_for_edit(&pool, id, &auth_user(owner_id, false))
        .await
        .unwrap();
    assert_eq!(shelf.id, id);
}

#[tokio::test]
async fn shelf_for_edit_allows_an_admin_who_is_not_the_owner() {
    let (pool, owner_id, other_id) = pool_with_two_users().await;
    let id = seed_shelf(&pool, owner_id, Visibility::Private).await;

    let shelf = shelf_for_edit(&pool, id, &auth_user(other_id, true))
        .await
        .unwrap();
    assert_eq!(shelf.id, id);
}

#[tokio::test]
async fn shelf_for_edit_denies_a_non_owner_non_admin_even_on_a_public_shelf() {
    let (pool, owner_id, other_id) = pool_with_two_users().await;
    let id = seed_shelf(&pool, owner_id, Visibility::Public).await;

    let err = shelf_for_edit(&pool, id, &auth_user(other_id, false))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("not your shelf"));
}
