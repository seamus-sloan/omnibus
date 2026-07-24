//! Tests for the physical-collection REST handlers: copy reads/edits, the
//! per-user wishlist entry, and fileless-book removal. Copies are library-wide,
//! so the write paths are gated on `can_edit` — `create_user` has it off and
//! `create_admin` has it on, which is what separates the 403s from the 200s.

use axum::{
    body::{to_bytes, Body},
    http::{header::AUTHORIZATION, Request, StatusCode},
};
use omnibus_db as db;
use omnibus_shared::physical::{PhysicalCopy, WishlistEntry, WishlistSource};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

fn req(method: &str, uri: &str, token: &str, body: Option<serde_json::Value>) -> Request<Body> {
    let b = Request::builder()
        .uri(uri)
        .method(method)
        .header(AUTHORIZATION, format!("Bearer {token}"));
    match body {
        Some(v) => b
            .header("content-type", "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => b.body(Body::empty()).unwrap(),
    }
}

async fn json_body<T: serde::de::DeserializeOwned>(res: axum::response::Response) -> T {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

/// A fileless book with one physical copy — the physical-only shape book detail
/// renders. Returns `(book uuid, copy id)`.
async fn seed_physical_only(pool: &sqlx::SqlitePool, title: &str) -> (String, i64) {
    let uuid = db::create_fileless_book(
        pool,
        db::FilelessBook {
            title: title.to_string(),
            authors: vec!["Ada Lovelace".into()],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .expect("create_fileless_book");
    let copy = db::add_physical_copy(pool, &uuid, None, None, Some("1st ed"))
        .await
        .expect("add_physical_copy");
    (uuid, copy.id)
}

#[tokio::test]
async fn api_get_physical_copies_requires_auth() {
    let (app, _state, _pool) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/physical/whatever/copies"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_physical_copies_returns_the_books_copies() {
    let _covers = CoversDirGuard::new("phys_list");
    let (app, _state, pool) = fixture().await;
    let (uuid, copy_id) = seed_physical_only(&pool, "Paper Only").await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/physical/{uuid}/copies"),
            &token,
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let copies: Vec<PhysicalCopy> = json_body(res).await;
    assert_eq!(copies.len(), 1);
    assert_eq!(copies[0].id, copy_id);
    assert_eq!(copies[0].note.as_deref(), Some("1st ed"));
}

#[tokio::test]
async fn api_get_physical_copies_returns_empty_for_unknown_book() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer("/api/physical/no-such-uuid/copies", &token))
        .await
        .unwrap();

    // A read of a book we don't have is an empty collection, not a 404.
    assert_eq!(res.status(), StatusCode::OK);
    let copies: Vec<PhysicalCopy> = json_body(res).await;
    assert!(copies.is_empty());
}

#[tokio::test]
async fn api_patch_copy_note_updates_the_note() {
    let _covers = CoversDirGuard::new("phys_patch");
    let (app, _state, pool) = fixture().await;
    let (_uuid, copy_id) = seed_physical_only(&pool, "Paper Only").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(req(
            "PATCH",
            &format!("/api/physical/copies/{copy_id}"),
            &token,
            Some(serde_json::json!({ "note": "signed 1st ed" })),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let copy: PhysicalCopy = json_body(res).await;
    assert_eq!(copy.note.as_deref(), Some("signed 1st ed"));
}

#[tokio::test]
async fn api_patch_copy_note_rejects_a_user_without_edit_permission() {
    let _covers = CoversDirGuard::new("phys_patch_403");
    let (app, _state, pool) = fixture().await;
    let (_uuid, copy_id) = seed_physical_only(&pool, "Paper Only").await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(req(
            "PATCH",
            &format!("/api/physical/copies/{copy_id}"),
            &token,
            Some(serde_json::json!({ "note": "mine now" })),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_patch_copy_note_returns_404_for_unknown_copy() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(req(
            "PATCH",
            "/api/physical/copies/9999",
            &token,
            Some(serde_json::json!({ "note": "x" })),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_delete_copy_removes_it() {
    let _covers = CoversDirGuard::new("phys_del_copy");
    let (app, _state, pool) = fixture().await;
    let (uuid, copy_id) = seed_physical_only(&pool, "Paper Only").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(req(
            "DELETE",
            &format!("/api/physical/copies/{copy_id}"),
            &token,
            None,
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert!(db::list_physical_copies(&pool, &uuid)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn api_delete_copy_returns_404_for_unknown_copy() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(req("DELETE", "/api/physical/copies/9999", &token, None))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_wishlist_entry_returns_null_when_not_wishlisted() {
    let _covers = CoversDirGuard::new("phys_wish_none");
    let (app, _state, pool) = fixture().await;
    let (uuid, _) = seed_physical_only(&pool, "Paper Only").await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/physical/{uuid}/wishlist"),
            &token,
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let entry: Option<WishlistEntry> = json_body(res).await;
    assert!(entry.is_none());
}

#[tokio::test]
async fn api_get_wishlist_entry_returns_the_callers_own_entry() {
    let _covers = CoversDirGuard::new("phys_wish_some");
    let (app, _state, pool) = fixture().await;
    let (uuid, _) = seed_physical_only(&pool, "Paper Only").await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    db::add_wishlist_entry(&pool, user.id, &uuid, WishlistSource::Detail)
        .await
        .unwrap();

    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/physical/{uuid}/wishlist"),
            &token,
        ))
        .await
        .unwrap();

    let entry: Option<WishlistEntry> = json_body(res).await;
    let entry = entry.expect("wishlisted book returns its entry");
    assert_eq!(entry.user_id, user.id);
    assert_eq!(entry.source, WishlistSource::Detail);
}

#[tokio::test]
async fn api_delete_wishlist_entry_is_idempotent() {
    let _covers = CoversDirGuard::new("phys_wish_del");
    let (app, _state, pool) = fixture().await;
    let (uuid, _) = seed_physical_only(&pool, "Paper Only").await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    // No entry to begin with: the desired end state already holds, so this is
    // a 204 rather than a 404.
    let res = app
        .oneshot(req(
            "DELETE",
            &format!("/api/physical/{uuid}/wishlist"),
            &token,
            None,
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn api_delete_book_removes_a_fileless_book() {
    let _covers = CoversDirGuard::new("phys_del_book");
    let (app, _state, pool) = fixture().await;
    let (uuid, _) = seed_physical_only(&pool, "Paper Only").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(req(
            "DELETE",
            &format!("/api/physical/{uuid}"),
            &token,
            None,
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert!(db::get_book_by_uuid(&pool, &uuid).await.unwrap().is_none());
}

#[tokio::test]
async fn api_delete_book_returns_409_when_the_book_still_has_files() {
    let (app, _state, pool) = fixture().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Has Files").await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(req(
            "DELETE",
            &format!("/api/physical/{uuid}"),
            &token,
            None,
        ))
        .await
        .unwrap();

    // A file-backed book belongs to the reindex diff, not this route.
    assert_eq!(res.status(), StatusCode::CONFLICT);
    assert!(db::get_book_by_uuid(&pool, &uuid).await.unwrap().is_some());
}

#[tokio::test]
async fn api_delete_book_rejects_a_user_without_edit_permission() {
    let _covers = CoversDirGuard::new("phys_del_book_403");
    let (app, _state, pool) = fixture().await;
    let (uuid, _) = seed_physical_only(&pool, "Paper Only").await;
    let user = auth_test_support::create_user(&pool, "reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let res = app
        .oneshot(req(
            "DELETE",
            &format!("/api/physical/{uuid}"),
            &token,
            None,
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::FORBIDDEN);
    assert!(db::get_book_by_uuid(&pool, &uuid).await.unwrap().is_some());
}

#[tokio::test]
async fn api_delete_book_returns_404_for_unknown_uuid() {
    let (app, _state, pool) = fixture().await;
    let admin = auth_test_support::create_admin(&pool, "admin").await;
    let token = auth_test_support::bearer_token(&pool, admin.id).await;

    let res = app
        .oneshot(req("DELETE", "/api/physical/no-such-uuid", &token, None))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
