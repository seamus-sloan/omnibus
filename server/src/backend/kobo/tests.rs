//! HTTP-layer contract tests for the wireless Kobo routes, driving
//! `kobo_router` via `oneshot` against an in-memory DB. The device sequence
//! (sync → metadata → state) is replayed at the HTTP layer because Playwright
//! can't drive a Kobo.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    response::Response,
    Router,
};
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use omnibus_shared::ReadStatus;
use serde_json::Value;
use sqlx::SqlitePool;
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;

/// Kobo router wired to a fresh in-memory DB, plus a valid path token and the
/// owning user's id (for read-state assertions).
async fn fixture() -> (Router, SqlitePool, String, i64) {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    let app = kobo_router(AppState::new(pool.clone()));
    let user = auth_test_support::create_user(&pool, "kobo-reader").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    (app, pool, token, user.id)
}

async fn body_json(res: Response) -> Value {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn get(uri: String) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .header("host", "omni.test")
        .body(Body::empty())
        .unwrap()
}

#[tokio::test]
async fn library_sync_rejects_an_invalid_token() {
    let (app, _pool, _token, _uid) = fixture().await;
    let res = app
        .oneshot(get("/kobo/not-a-real-token/v1/library/sync".to_owned()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn library_sync_streams_every_book_with_no_cap() {
    let (app, pool, token, _uid) = fixture().await;
    for i in 0..150 {
        seed_synced_ebook(
            &pool,
            &format!("b{i}.epub"),
            &format!("Title {i}"),
            "Author",
        )
        .await;
    }
    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers().get("x-kobo-synctoken").unwrap(), "slice-a");
    let json = body_json(res).await;
    // Deliberately never adopts Calibre-Web's SYNC_ITEM_LIMIT=100 cap.
    assert_eq!(json.as_array().unwrap().len(), 150);
}

#[tokio::test]
async fn library_sync_emits_new_entitlement_pointing_at_download() {
    let (app, pool, token, _uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    let json = body_json(res).await;
    let ent = &json.as_array().unwrap()[0]["NewEntitlement"];
    assert_eq!(ent["BookEntitlement"]["Id"], uuid);
    assert_eq!(ent["BookMetadata"]["Title"], "Dune");
    assert_eq!(
        ent["BookMetadata"]["ContributorRoles"][0]["Name"],
        "Frank Herbert"
    );
    let url = ent["BookMetadata"]["DownloadUrls"][0]["Url"]
        .as_str()
        .unwrap();
    assert!(
        url.contains(&token),
        "download url should carry the path token"
    );
    assert!(
        url.contains(&uuid),
        "download url should carry the book uuid"
    );
}

#[tokio::test]
async fn metadata_returns_the_book() {
    let (app, pool, token, _uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "gatsby.epub", "The Great Gatsby", "Fitzgerald").await;
    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/{uuid}/metadata")))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    assert_eq!(json[0]["Title"], "The Great Gatsby");
}

#[tokio::test]
async fn metadata_returns_404_for_unknown_uuid() {
    let (app, _pool, token, _uid) = fixture().await;
    let res = app
        .oneshot(get(format!(
            "/kobo/{token}/v1/library/does-not-exist/metadata"
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn put_state_persists_read_status_and_returns_success() {
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "solaris.epub", "Solaris", "Lem").await;
    let body = serde_json::json!({
        "ReadingStates": [{
            "StatusInfo": { "Status": "Finished" },
            "CurrentBookmark": { "ProgressPercent": 100 }
        }]
    });
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/kobo/{token}/v1/library/{uuid}/state"))
                .method("PUT")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    assert_eq!(json["RequestResult"], "Success");

    let rec = db::read_status::get_read_status(&pool, uid, &uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.status, ReadStatus::Finished);
}

#[tokio::test]
async fn download_returns_404_for_unknown_uuid() {
    let (app, _pool, token, _uid) = fixture().await;
    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/download/does-not-exist")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn library_tags_returns_an_empty_collection() {
    let (app, _pool, token, _uid) = fixture().await;
    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/tags")))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    assert!(json.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn image_returns_404_for_unknown_uuid() {
    let (app, _pool, token, _uid) = fixture().await;
    let res = app
        .oneshot(get(format!(
            "/kobo/{token}/v1/books/nope/thumbnail/400/600/100/false/image.jpg"
        )))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}
