//! The initialization resources map a device fetches first, the store
//! endpoints left pointed at Kobo, the empty store stub, and the empty
//! library-tags collection.

use axum::http::StatusCode;
use tower::ServiceExt;

use super::{body_json, fixture, get};

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
async fn store_stub_answers_firmware_hardcoded_paths_with_an_empty_object() {
    // The device derives these from `api_endpoint` directly, bypassing the
    // resources map; each must get a benign 200 or the sync aborts.
    let (app, _pool, token, _uid) = fixture().await;
    for path in [
        "v1/user/profile",
        "v1/user/loyalty/benefits",
        "v1/products/books/subscriptions",
        "v1/deals",
    ] {
        let res = app
            .clone()
            .oneshot(get(format!("/kobo/{token}/{path}")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK, "{path}");
        assert_eq!(body_json(res).await, serde_json::json!({}), "{path}");
    }
}

#[tokio::test]
async fn store_stub_rejects_an_invalid_token() {
    let (app, _pool, _token, _uid) = fixture().await;
    let res = app
        .oneshot(get("/kobo/not-a-real-token/v1/user/profile".to_owned()))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn store_stub_does_not_shadow_registered_routes() {
    // The wildcard must lose to every registered route: initialization keeps
    // its real payload rather than the stub's empty object.
    let (app, _pool, token, _uid) = fixture().await;
    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/initialization")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(body_json(res).await.get("Resources").is_some());
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
