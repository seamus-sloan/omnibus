//! Tests for the `/api/covers/*` and `/api/thumbs/*` REST handlers.
use axum::{body::to_bytes, http::StatusCode};
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_get_cover_returns_404_when_book_not_found() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let response = app
        .oneshot(get_with_bearer("/api/covers/9999", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_cover_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(get_anon("/api/covers/1")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_cover_returns_404_when_book_exists_but_has_no_cover_on_record() {
    let (app, _, pool) = fixture().await;
    // seed_book_no_cover uses this fixed uuid; route is uuid-keyed.
    let _ = seed_book_no_cover(&pool).await;
    let uuid = "00000000-0000-0000-0000-000000000001";
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(&format!("/api/covers/{uuid}"), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_cover_returns_200_with_image_bytes_for_existing_cover() {
    let (app, _, pool) = fixture().await;
    let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Cover Book").await;
    sqlx::query("UPDATE books SET has_cover = 1 WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let _covers_guard = CoversDirGuard::new("get_cover_200");
    std::fs::write(db::covers_dir().join(format!("{uuid}.png")), TINY_PNG)
        .expect("write cover fixture");
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(&format!("/api/covers/{uuid}"), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], TINY_PNG);
}

#[tokio::test]
async fn api_get_cover_returns_304_when_if_none_match_matches_the_current_etag() {
    let (app, _, pool) = fixture().await;
    let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Cover Book").await;
    sqlx::query("UPDATE books SET has_cover = 1 WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let _covers_guard = CoversDirGuard::new("get_cover_304");
    std::fs::write(db::covers_dir().join(format!("{uuid}.png")), TINY_PNG)
        .expect("write cover fixture");
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let first = app
        .clone()
        .oneshot(get_with_bearer(&format!("/api/covers/{uuid}"), &token))
        .await
        .unwrap();
    assert_eq!(first.status(), StatusCode::OK);
    let etag = first
        .headers()
        .get(header::ETAG)
        .expect("first response should carry an ETag")
        .to_str()
        .unwrap()
        .to_string();

    // Same bytes on disk: a client that already cached this ETag should
    // get a bodyless 304, not a fresh copy of the image (#1086 — the
    // revalidation itself must be cheap even though every load now
    // triggers one).
    let second = app
        .oneshot(get_with_bearer_and_if_none_match(
            &format!("/api/covers/{uuid}"),
            &token,
            &etag,
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(second.headers().get(header::ETAG).unwrap(), etag.as_str());
    // The 304 must carry the same revalidation-forcing Cache-Control as the
    // 200 it stands in for — a regression that dropped it here would let a
    // client silently fall back to caching this response for a full day.
    assert_eq!(
        second.headers().get(header::CACHE_CONTROL).unwrap(),
        "private, no-cache"
    );
    let bytes = to_bytes(second.into_body(), usize::MAX).await.unwrap();
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn api_get_cover_sets_identical_cookie_and_authorization_vary_on_the_200_and_304() {
    let (app, _, pool) = fixture().await;
    let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Cover Book").await;
    sqlx::query("UPDATE books SET has_cover = 1 WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let _covers_guard = CoversDirGuard::new("get_cover_vary");
    std::fs::write(db::covers_dir().join(format!("{uuid}.png")), TINY_PNG)
        .expect("write cover fixture");
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let first = app
        .clone()
        .oneshot(get_with_bearer(&format!("/api/covers/{uuid}"), &token))
        .await
        .unwrap();
    // `MediaAuthUser` accepts a session cookie *or* a bearer header, so a
    // shared/intermediate cache that varies only on `Cookie` could serve
    // one bearer-authenticated user's response to another. Both `Cookie`
    // and `Authorization` must be present here.
    assert_eq!(
        first.headers().get(header::VARY).unwrap(),
        "Cookie, Authorization"
    );
    let etag = first
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    let second = app
        .oneshot(get_with_bearer_and_if_none_match(
            &format!("/api/covers/{uuid}"),
            &token,
            &etag,
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    // The regression this guards against: a 304 path that forgot to carry
    // the same `Vary` as its 200 counterpart would silently reopen the
    // cross-user cache gap even though the 200 path looks correct.
    assert_eq!(
        second.headers().get(header::VARY).unwrap(),
        first.headers().get(header::VARY).unwrap()
    );
}

#[tokio::test]
async fn api_get_cover_serves_fresh_bytes_and_a_new_etag_after_the_cover_changes() {
    let (app, _, pool) = fixture().await;
    let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Cover Book").await;
    sqlx::query("UPDATE books SET has_cover = 1 WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let _covers_guard = CoversDirGuard::new("get_cover_etag_changes");
    let cover_path = db::covers_dir().join(format!("{uuid}.png"));
    std::fs::write(&cover_path, TINY_PNG).expect("write cover fixture");
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let first = app
        .clone()
        .oneshot(get_with_bearer(&format!("/api/covers/{uuid}"), &token))
        .await
        .unwrap();
    let stale_etag = first
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();

    // Simulate a book-editor cover replace: new bytes land at the same
    // uuid-keyed path. A request still carrying the pre-change ETag must
    // NOT be told "not modified" — it needs the new image (this is the
    // regression this fix targets: reloading/revisiting after a cover
    // update must show the update, per #1086 AC2).
    std::fs::write(&cover_path, [TINY_PNG, b"-changed"].concat()).expect("overwrite cover fixture");

    let second = app
        .oneshot(get_with_bearer_and_if_none_match(
            &format!("/api/covers/{uuid}"),
            &token,
            &stale_etag,
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::OK);
    let fresh_etag = second
        .headers()
        .get(header::ETAG)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert_ne!(fresh_etag, stale_etag);
    let bytes = to_bytes(second.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], [TINY_PNG, b"-changed"].concat().as_slice());
}

#[tokio::test]
async fn api_get_cover_sets_private_no_cache_cache_control_so_clients_always_revalidate() {
    let (app, _, pool) = fixture().await;
    let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Cover Book").await;
    sqlx::query("UPDATE books SET has_cover = 1 WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let _covers_guard = CoversDirGuard::new("get_cover_cache_control");
    std::fs::write(db::covers_dir().join(format!("{uuid}.png")), TINY_PNG)
        .expect("write cover fixture");
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(&format!("/api/covers/{uuid}"), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CACHE_CONTROL).unwrap(),
        "private, no-cache"
    );
}

#[tokio::test]
async fn api_get_cover_returns_500_when_metadata_overrides_table_is_missing() {
    let (app, _, pool) = fixture().await;
    let (_id, uuid) = seed_book_with_uuid(&pool, "/lib", "Cover Book").await;
    sqlx::query("DROP TABLE metadata_overrides")
        .execute(&pool)
        .await
        .unwrap();
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(&format!("/api/covers/{uuid}"), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn api_get_cover_returns_500_when_books_table_is_missing_during_uuid_resolution() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();
    let res = app
        .oneshot(get_with_bearer("/api/covers/any-uuid", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn api_get_thumb_returns_400_for_bad_size() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/thumbs/1/xxl", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn api_get_thumb_returns_404_for_missing_book() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/thumbs/9999/md", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_thumb_returns_202_for_book_without_cover() {
    let (_, _, pool) = fixture().await;
    // seed_book_no_cover uses this fixed uuid; route is uuid-keyed now.
    let _ = seed_book_no_cover(&pool).await;
    let uuid = "00000000-0000-0000-0000-000000000001";
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(&format!("/api/thumbs/{uuid}/md"), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
}

#[tokio::test]
async fn api_get_thumb_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(get_anon("/api/thumbs/1/md")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_thumb_returns_200_and_serves_cached_webp_on_cache_hit() {
    let (app, _, pool) = fixture().await;
    let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Thumb Book").await;
    // Force `last_modified` far in the past so the thumb file we drop in
    // below (written with the real current mtime) reads as fresh.
    sqlx::query("UPDATE books SET last_modified = 0 WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let _thumbs_guard = ThumbsDirGuard::new("thumb_cache_hit");
    let thumb_bytes = b"fake-cached-webp-bytes";
    std::fs::write(db::thumb_path_for(id, db::ThumbSize::Md), thumb_bytes)
        .expect("write thumb fixture");
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(&format!("/api/thumbs/{uuid}/md"), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/webp"
    );
    assert_eq!(
        res.headers().get(header::CACHE_CONTROL).unwrap(),
        "private, no-cache"
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], thumb_bytes);
}

#[tokio::test]
async fn api_get_thumb_returns_304_when_if_none_match_matches_the_cached_webps_etag() {
    let (app, _, pool) = fixture().await;
    let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Thumb Book").await;
    sqlx::query("UPDATE books SET last_modified = 0 WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let _thumbs_guard = ThumbsDirGuard::new("thumb_cache_hit_304");
    std::fs::write(
        db::thumb_path_for(id, db::ThumbSize::Md),
        b"fake-cached-webp-bytes",
    )
    .expect("write thumb fixture");
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let first = app
        .clone()
        .oneshot(get_with_bearer(&format!("/api/thumbs/{uuid}/md"), &token))
        .await
        .unwrap();
    let etag = first
        .headers()
        .get(header::ETAG)
        .expect("cache-hit response should carry an ETag")
        .to_str()
        .unwrap()
        .to_string();

    // Same rationale as the cover 304 test: a client revisiting/reloading
    // with an up-to-date thumb should get a bodyless 304, not a refetch of
    // bytes it already has (#1086).
    let second = app
        .oneshot(get_with_bearer_and_if_none_match(
            &format!("/api/thumbs/{uuid}/md"),
            &token,
            &etag,
        ))
        .await
        .unwrap();
    assert_eq!(second.status(), StatusCode::NOT_MODIFIED);
    // Same regression guard as the cover 304 test: the 304 must repeat the
    // 200's `ETag` and revalidation-forcing `Cache-Control`, not just its
    // status code.
    assert_eq!(second.headers().get(header::ETAG).unwrap(), etag.as_str());
    assert_eq!(
        second.headers().get(header::CACHE_CONTROL).unwrap(),
        "private, no-cache"
    );
    let bytes = to_bytes(second.into_body(), usize::MAX).await.unwrap();
    assert!(bytes.is_empty());
}

#[tokio::test]
async fn api_get_thumb_returns_200_and_serves_original_cover_on_cache_miss() {
    let (app, _, pool) = fixture().await;
    let (id, uuid) = seed_book_with_uuid(&pool, "/lib", "Thumb Miss Book").await;
    sqlx::query("UPDATE books SET has_cover = 1 WHERE id = ?")
        .bind(id)
        .execute(&pool)
        .await
        .unwrap();
    let _covers_guard = CoversDirGuard::new("thumb_cache_miss_cover");
    let _thumbs_guard = ThumbsDirGuard::new("thumb_cache_miss");
    std::fs::write(db::covers_dir().join(format!("{uuid}.png")), TINY_PNG)
        .expect("write cover fixture");
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(&format!("/api/thumbs/{uuid}/md"), &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(header::CONTENT_TYPE).unwrap(),
        "image/png"
    );
    assert_eq!(
        res.headers().get(header::CACHE_CONTROL).unwrap(),
        "private, max-age=5"
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], TINY_PNG);
}

#[tokio::test]
async fn api_get_thumb_returns_500_when_books_table_is_missing_during_uuid_resolution() {
    let (app, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();
    let res = app
        .oneshot(get_with_bearer("/api/thumbs/any-uuid/md", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
