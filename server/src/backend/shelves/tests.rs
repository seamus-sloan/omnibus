//! Integration tests for the shelf REST handlers — HTTP contract and
//! visibility/authorization. Membership logic is covered by the db suite.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_shared::{Shelf, ShelfSummary};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

fn req(
    method: &str,
    uri: &str,
    token: Option<&str>,
    body: Option<serde_json::Value>,
) -> Request<Body> {
    let mut b = Request::builder().uri(uri).method(method);
    if let Some(t) = token {
        b = b.header(AUTHORIZATION, format!("Bearer {t}"));
    }
    let body = match body {
        Some(v) => {
            b = b.header("content-type", "application/json");
            Body::from(v.to_string())
        }
        None => Body::empty(),
    };
    b.body(body).unwrap()
}

async fn shelf_from(res: axum::response::Response) -> Shelf {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn smart_body(name: &str, visibility: &str) -> serde_json::Value {
    serde_json::json!({
        "kind": "smart",
        "name": name,
        "visibility": visibility,
        "match_mode": "any",
        "rules": [{ "field": "tag", "op": "is", "value": "Fantasy" }],
    })
}

/// Seed one book row directly so hand-picked membership tests have a real uuid.
async fn seed_book(pool: &sqlx::SqlitePool, uuid: &str) {
    let lib: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib') RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap();
    let book: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort)
         VALUES (?, ?, ?, '/lib/b', 'B', 'B') RETURNING id",
    )
    .bind(uuid)
    .bind(uuid)
    .bind(lib)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch)
         VALUES (?, 'EPUB', 'b', 1, 1)",
    )
    .bind(book)
    .execute(pool)
    .await
    .unwrap();
}

#[tokio::test]
async fn api_list_shelves_requires_auth() {
    let (app, _s, _p) = fixture().await;
    let res = app
        .oneshot(req("GET", "/api/shelves", None, None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_create_smart_shelf_returns_201() {
    let (app, _s, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(req(
            "POST",
            "/api/shelves",
            Some(&token),
            Some(smart_body("Fantasy", "private")),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::CREATED);
    let shelf = shelf_from(res).await;
    assert_eq!(shelf.name, "Fantasy");
    assert_eq!(shelf.rules.len(), 1);
}

#[tokio::test]
async fn api_create_duplicate_name_returns_409() {
    let (app, _s, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let first = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/shelves",
            Some(&token),
            Some(smart_body("Dup", "private")),
        ))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::CREATED);

    let second = app
        .oneshot(req(
            "POST",
            "/api/shelves",
            Some(&token),
            Some(smart_body("dup", "private")),
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::CONFLICT);
}

#[tokio::test]
async fn api_create_smart_without_rules_returns_422() {
    let (app, _s, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let body = serde_json::json!({
        "kind": "smart", "name": "Empty", "visibility": "private",
        "match_mode": "any", "rules": [],
    });
    let res = app
        .oneshot(req("POST", "/api/shelves", Some(&token), Some(body)))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn api_private_shelf_hidden_from_non_owner_visible_to_admin() {
    let (app, _s, pool) = fixture().await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let alice_token = auth_test_support::bearer_token(&pool, alice.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let bob_token = auth_test_support::bearer_token(&pool, bob.id).await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let admin_token = auth_test_support::bearer_token(&pool, admin.id).await;

    let created = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/shelves",
            Some(&alice_token),
            Some(smart_body("Secret", "private")),
        ))
        .await
        .unwrap();
    let id = shelf_from(created).await.id;
    let uri = format!("/api/shelves/{id}");

    let owner = app
        .clone()
        .oneshot(req("GET", &uri, Some(&alice_token), None))
        .await
        .unwrap();
    assert_eq!(owner.status(), StatusCode::OK);
    let other = app
        .clone()
        .oneshot(req("GET", &uri, Some(&bob_token), None))
        .await
        .unwrap();
    assert_eq!(other.status(), StatusCode::NOT_FOUND);
    let by_admin = app
        .oneshot(req("GET", &uri, Some(&admin_token), None))
        .await
        .unwrap();
    assert_eq!(by_admin.status(), StatusCode::OK);
}

#[tokio::test]
async fn api_public_shelf_visible_but_not_editable_by_others() {
    let (app, _s, pool) = fixture().await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let alice_token = auth_test_support::bearer_token(&pool, alice.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let bob_token = auth_test_support::bearer_token(&pool, bob.id).await;

    let created = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/shelves",
            Some(&alice_token),
            Some(smart_body("Shared", "public")),
        ))
        .await
        .unwrap();
    let id = shelf_from(created).await.id;
    let uri = format!("/api/shelves/{id}");

    // Bob can see it…
    let view = app
        .clone()
        .oneshot(req("GET", &uri, Some(&bob_token), None))
        .await
        .unwrap();
    assert_eq!(view.status(), StatusCode::OK);
    // …but cannot edit it.
    let patch = app
        .clone()
        .oneshot(req(
            "PATCH",
            &uri,
            Some(&bob_token),
            Some(serde_json::json!({ "name": "Hijack" })),
        ))
        .await
        .unwrap();
    assert_eq!(patch.status(), StatusCode::FORBIDDEN);
    // …nor delete it.
    let del = app
        .oneshot(req("DELETE", &uri, Some(&bob_token), None))
        .await
        .unwrap();
    assert_eq!(del.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_list_returns_only_visible_shelves() {
    let (app, _s, pool) = fixture().await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let alice_token = auth_test_support::bearer_token(&pool, alice.id).await;
    let bob = auth_test_support::create_user(&pool, "bob").await;
    let bob_token = auth_test_support::bearer_token(&pool, bob.id).await;

    app.clone()
        .oneshot(req(
            "POST",
            "/api/shelves",
            Some(&alice_token),
            Some(smart_body("Priv", "private")),
        ))
        .await
        .unwrap();
    app.clone()
        .oneshot(req(
            "POST",
            "/api/shelves",
            Some(&alice_token),
            Some(smart_body("Pub", "public")),
        ))
        .await
        .unwrap();

    let res = app
        .oneshot(req("GET", "/api/shelves", Some(&bob_token), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let shelves: Vec<ShelfSummary> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(shelves.len(), 1);
    assert_eq!(shelves[0].name, "Pub");
}

#[tokio::test]
async fn api_add_book_rejects_oversized_book_uuids_array_with_422() {
    let (app, _s, pool) = fixture().await;
    seed_book(&pool, "bk1").await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, alice.id).await;

    let created = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/shelves",
            Some(&token),
            Some(serde_json::json!({ "kind": "manual", "name": "Picks", "visibility": "private" })),
        ))
        .await
        .unwrap();
    let id = shelf_from(created).await.id;

    let too_many: Vec<String> = (0..omnibus_shared::MAX_SHELF_BOOK_UUIDS + 1)
        .map(|i| i.to_string())
        .collect();
    let res = app
        .oneshot(req(
            "POST",
            &format!("/api/shelves/{id}/books"),
            Some(&token),
            Some(serde_json::json!({ "book_uuids": too_many })),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn api_preview_rule_rejects_too_many_rules_with_422() {
    let (app, _s, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let rules: Vec<_> = std::iter::repeat_n(
        serde_json::json!({ "field": "tag", "op": "is", "value": "Fantasy" }),
        omnibus_shared::MAX_PREVIEW_RULES + 1,
    )
    .collect();
    let body = serde_json::json!({ "match_mode": "any", "rules": rules });
    let res = app
        .oneshot(req(
            "POST",
            "/api/shelves/preview",
            Some(&token),
            Some(body),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
}

#[tokio::test]
async fn api_add_book_to_manual_shelf_returns_204() {
    let (app, _s, pool) = fixture().await;
    seed_book(&pool, "bk1").await;
    let alice = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, alice.id).await;

    let created = app
        .clone()
        .oneshot(req(
            "POST",
            "/api/shelves",
            Some(&token),
            Some(serde_json::json!({ "kind": "manual", "name": "Picks", "visibility": "private" })),
        ))
        .await
        .unwrap();
    let id = shelf_from(created).await.id;

    let res = app
        .oneshot(req(
            "POST",
            &format!("/api/shelves/{id}/books"),
            Some(&token),
            Some(serde_json::json!({ "book_uuids": ["bk1"] })),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
}

/// The owner's provisioned wishlist shelf id (server test users are inserted
/// raw, so provision it explicitly like `create_user` / the boot backfill do).
async fn wishlist_shelf_id(pool: &sqlx::SqlitePool, owner: i64) -> i64 {
    omnibus_db::provision_wishlist_shelf(pool, owner)
        .await
        .unwrap();
    sqlx::query_scalar::<_, i64>(
        "SELECT id FROM shelves WHERE owner_user_id = ? AND kind = 'wishlist'",
    )
    .bind(owner)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn api_delete_wishlist_system_shelf_returns_403() {
    let (app, _s, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let id = wishlist_shelf_id(&pool, user.id).await;

    let res = app
        .oneshot(req(
            "DELETE",
            &format!("/api/shelves/{id}"),
            Some(&token),
            None,
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_update_wishlist_system_shelf_returns_403() {
    let (app, _s, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let id = wishlist_shelf_id(&pool, user.id).await;

    let res = app
        .oneshot(req(
            "PATCH",
            &format!("/api/shelves/{id}"),
            Some(&token),
            Some(serde_json::json!({ "name": "Renamed" })),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}
