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

/// Put `uuids` on a hand-picked shelf owned by `user_id` and flag it for Kobo
/// sync. Since #924 the sync set is shelf-gated, so any test that expects a
/// book back from `library/sync` must opt it in first.
async fn opt_in(pool: &SqlitePool, user_id: i64, uuids: &[String]) {
    let shelf = db::shelves::create_shelf(
        pool,
        user_id,
        &omnibus_shared::CreateShelfRequest {
            kind: omnibus_shared::ShelfKind::Manual,
            name: "Kobo".into(),
            description: None,
            visibility: Default::default(),
            match_mode: None,
            rules: Vec::new(),
            book_uuids: uuids.to_vec(),
        },
    )
    .await
    .unwrap();
    db::shelves::update_shelf(
        pool,
        shelf.id,
        &omnibus_shared::UpdateShelfRequest {
            sync_to_kobo: Some(true),
            ..Default::default()
        },
    )
    .await
    .unwrap();
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
    let (app, pool, token, uid) = fixture().await;
    let mut uuids = Vec::new();
    for i in 0..150 {
        uuids.push(
            seed_synced_ebook(
                &pool,
                &format!("b{i}.epub"),
                &format!("Title {i}"),
                "Author",
            )
            .await,
        );
    }
    opt_in(&pool, uid, &uuids).await;
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
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Frank Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
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

#[test]
fn auth_envelope_values_are_pinned_to_a_fixed_digest() {
    // Hard-coded rather than recomputed: the whole point is that these values
    // survive a toolchain bump. A hash-function swap that silently rotated
    // every device's envelope would pass a self-consistent test but fail here.
    let env = dto::auth_envelope("tok123");

    // == sha256(b"access" + b"\x00" + b"tok123"), verified independently.
    assert_eq!(
        env.access_token,
        "ecaf3802245d9f7c9914df62bdb3baa89388b6a6cb08d4b703dcb696c6bcd220"
    );
    assert_eq!(env.token_type, "Bearer");
    assert_ne!(env.access_token, env.refresh_token);
    assert_ne!(env.tracking_id, env.user_key);
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
async fn library_sync_returns_nothing_when_no_shelf_is_opted_in() {
    // AC1: an indexed library with no `sync_to_kobo` shelf syncs nothing. The
    // gate is the default, not an opt-out.
    let (app, pool, token, _uid) = fixture().await;
    seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert!(body_json(res).await.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn library_sync_excludes_a_book_on_an_unflagged_shelf() {
    // AC1: shelf membership alone is not enough — the shelf must be flagged.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    db::shelves::create_shelf(
        &pool,
        uid,
        &omnibus_shared::CreateShelfRequest {
            kind: omnibus_shared::ShelfKind::Manual,
            name: "Not synced".into(),
            description: None,
            visibility: Default::default(),
            match_mode: None,
            rules: Vec::new(),
            book_uuids: vec![uuid],
        },
    )
    .await
    .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    assert!(body_json(res).await.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn library_sync_never_returns_another_users_opted_in_books() {
    // AC4: the opt-in is scoped through shelf ownership, so one user's flagged
    // shelf is invisible to another user's device token.
    let (app, pool, token, _uid) = fixture().await;
    let other = auth_test_support::create_user(&pool, "other-reader").await;
    let theirs = seed_synced_ebook(&pool, "theirs.epub", "Theirs", "B").await;
    opt_in(&pool, other.id, &[theirs]).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    assert!(body_json(res).await.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn library_sync_reflects_an_opt_in_toggled_off() {
    // AC2: toggling the flag changes what the *next* sync returns, with no
    // intermediate publish step.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, &[uuid]).await;

    let first = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(body_json(first).await.as_array().unwrap().len(), 1);

    let shelves = db::shelves::list_visible_shelves(&pool, uid, false)
        .await
        .unwrap();
    let shelf_id = shelves
        .iter()
        .find(|s| s.name == "Kobo")
        .expect("seeded shelf")
        .id;
    db::shelves::update_shelf(
        &pool,
        shelf_id,
        &omnibus_shared::UpdateShelfRequest {
            sync_to_kobo: Some(false),
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let second = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert!(body_json(second).await.as_array().unwrap().is_empty());
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
