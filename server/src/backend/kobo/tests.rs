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
/// owning user's id (for read-state assertions). The token is a real per-device
/// `kobo_devices` credential (#923), not a session token.
async fn fixture() -> (Router, SqlitePool, String, i64) {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    let app = kobo_router(AppState::new(pool.clone()));
    let user = auth_test_support::create_user(&pool, "kobo-reader").await;
    let device = db::kobo_devices::create_device(&pool, user.id, "Test Kobo")
        .await
        .unwrap();
    (app, pool, device.token, user.id)
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
async fn initialization_returns_the_resources_map_with_the_api_token_header() {
    // AC1: the header is load-bearing — without it the device rejects the
    // payload and never adopts the map.
    let (app, _pool, token, _uid) = fixture().await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/initialization")))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers().get("x-kobo-apitoken").unwrap(), "e30=");
    let json = body_json(res).await;
    assert!(json["Resources"].is_object());
}

#[tokio::test]
async fn initialization_points_sync_and_covers_at_this_server() {
    // AC2: the overridden entries resolve to the request origin and carry the
    // caller's path token.
    let (app, _pool, token, _uid) = fixture().await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/initialization")))
        .await
        .unwrap();
    let json = body_json(res).await;
    let r = &json["Resources"];

    assert_eq!(
        r["library_sync"].as_str().unwrap(),
        format!("http://omni.test/kobo/{token}/v1/library/sync")
    );
    assert_eq!(r["image_host"].as_str().unwrap(), "http://omni.test");
    assert!(r["image_url_template"].as_str().unwrap().contains(&token));
    assert_eq!(
        r["reading_services_host"].as_str().unwrap(),
        "http://omni.test"
    );
}

#[tokio::test]
async fn initialization_leaves_the_store_endpoints_pointed_at_kobo() {
    // AC3 (the pass-through half): store browse/search stay Kobo's, which is
    // what keeps them working without this server proxying anything.
    let (app, _pool, token, _uid) = fixture().await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/initialization")))
        .await
        .unwrap();
    let json = body_json(res).await;

    assert_eq!(
        json["Resources"]["products"].as_str().unwrap(),
        "https://storeapi.kobo.com/v1/products"
    );
}

#[tokio::test]
async fn initialization_rejects_an_invalid_token() {
    let (app, _pool, _token, _uid) = fixture().await;
    let res = app
        .oneshot(get("/kobo/not-a-real-token/v1/initialization".to_owned()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn auth_device_returns_a_bearer_envelope() {
    let (app, _pool, token, _uid) = fixture().await;

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/kobo/{token}/v1/auth/device"))
                .method("POST")
                .header("host", "omni.test")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    assert_eq!(json["TokenType"], "Bearer");
    for field in ["AccessToken", "RefreshToken", "TrackingId", "UserKey"] {
        assert!(
            json[field].as_str().is_some_and(|s| !s.is_empty()),
            "{field} should be present and non-empty"
        );
    }
}

#[tokio::test]
async fn auth_refresh_returns_the_same_envelope_as_the_initial_exchange() {
    // The device refreshes on a schedule; a value that changed between the two
    // would look like a rotated credential it needs to act on.
    let (app, _pool, token, _uid) = fixture().await;

    let device = app
        .clone()
        .oneshot(
            Request::builder()
                .uri(format!("/kobo/{token}/v1/auth/device"))
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    let refresh = app
        .oneshot(
            Request::builder()
                .uri(format!("/kobo/{token}/v1/auth/refresh"))
                .method("POST")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(body_json(device).await, body_json(refresh).await);
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
async fn a_revoked_token_is_rejected() {
    let (app, pool, token, uid) = fixture().await;
    // Revoke the only device this token belongs to.
    let dev = db::kobo_devices::list_devices(&pool, uid).await.unwrap();
    db::kobo_devices::revoke_device(&pool, uid, dev[0].id)
        .await
        .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn state_push_is_scoped_to_the_tokens_owner() {
    // AC4: a device token authorizes only its owner's user-scoped state — a
    // second user's token records read status under the second user, never the
    // first.
    let (app, pool, _owner_token, owner_id) = fixture().await;
    let other = auth_test_support::create_user(&pool, "other-reader").await;
    let other_token = db::kobo_devices::create_device(&pool, other.id, "Other Kobo")
        .await
        .unwrap()
        .token;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    let body = serde_json::json!({
        "ReadingStates": [{ "StatusInfo": { "Status": "Finished" } }]
    });
    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/kobo/{other_token}/v1/library/{uuid}/state"))
                .method("PUT")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    // Recorded under `other`, not the fixture's owner.
    assert!(db::read_status::get_read_status(&pool, other.id, &uuid)
        .await
        .unwrap()
        .is_some());
    assert!(db::read_status::get_read_status(&pool, owner_id, &uuid)
        .await
        .unwrap()
        .is_none());
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
