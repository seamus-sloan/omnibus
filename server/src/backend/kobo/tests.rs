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
async fn library_sync_delivers_every_book_across_the_continue_loop() {
    // 150 books > SYNC_PAGE_SIZE (100): exercises the real continue loop, unlike Calibre-Web's SYNC_ITEM_LIMIT nothing is dropped.
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

    let mut total = 0;
    let mut pages = 0;
    loop {
        let res = app
            .clone()
            .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers().get("x-kobo-synctoken").unwrap(), "omnibus");
        let more = res.headers().get("x-kobo-sync").is_some();
        if more {
            assert_eq!(res.headers().get("x-kobo-sync").unwrap(), "continue");
        }
        total += body_json(res).await.as_array().unwrap().len();
        pages += 1;
        assert!(pages <= 3, "continue loop failed to terminate");
        if !more {
            break;
        }
    }

    assert_eq!(total, 150);
    assert_eq!(pages, 2, "150 books should page as 100 + 50");
}

#[tokio::test]
async fn library_sync_omits_the_continue_header_when_one_page_suffices() {
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    assert!(res.headers().get("x-kobo-sync").is_none());
    assert_eq!(body_json(res).await.as_array().unwrap().len(), 1);
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
    // intermediate publish step. Since the per-device delta (#922) the device
    // is told to *archive* the book — a `ChangedEntitlement{IsRemoved:true}` —
    // rather than just no longer seeing it.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;

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
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    let items = body_json(second).await;
    let arr = items.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    let removed = &arr[0]["ChangedEntitlement"]["BookEntitlement"];
    assert_eq!(removed["Id"], uuid);
    assert_eq!(removed["IsRemoved"], true);

    // And once the removal is delivered, the third sync is a true no-op.
    let third = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert!(body_json(third).await.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn library_sync_returns_an_empty_delta_once_the_device_is_current() {
    // The snapshot advances when the body drains, so an unchanged library
    // yields an empty second sync instead of a full re-download.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;

    let first = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(body_json(first).await.as_array().unwrap().len(), 1);

    let second = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert!(body_json(second).await.as_array().unwrap().is_empty());
}

#[tokio::test]
async fn library_sync_emits_the_change_pair_when_a_book_is_modified() {
    // A modified book re-syncs as ChangedProductMetadata + ChangedReadingState
    // — never a duplicate NewEntitlement, which would double the shelf row on
    // the device.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    let first = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(body_json(first).await.as_array().unwrap().len(), 1);

    sqlx::query("UPDATE books SET last_modified = 9999999999 WHERE uuid = ?")
        .bind(&uuid)
        .execute(&pool)
        .await
        .unwrap();

    let second = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    let items = body_json(second).await;
    let arr = items.as_array().unwrap();
    assert_eq!(arr.len(), 2);
    assert_eq!(
        arr[0]["ChangedProductMetadata"]["BookMetadata"]["EntitlementId"],
        uuid
    );
    assert_eq!(
        arr[1]["ChangedReadingState"]["ReadingState"]["EntitlementId"],
        uuid
    );
}

/// POST a JSON body to a kobo route.
fn post_json(uri: String, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("host", "omni.test")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn analytics_leave_content_records_a_reading_session() {
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    let body = serde_json::json!({
        "Events": [{
            "Id": "evt-1",
            "EventType": "LeaveContent",
            "Timestamp": "2026-07-26T12:00:00Z",
            "Metrics": { "SecondsRead": 600 },
            "Attributes": { "volumeid": uuid }
        }]
    });
    let res = app
        .oneshot(post_json(format!("/kobo/{token}/v1/analytics/event"), body))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(res).await["Result"], "Success");

    let (seconds, client_id): (i64, String) =
        sqlx::query_as("SELECT seconds_read, client_id FROM reading_sessions WHERE user_id = ?")
            .bind(uid)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(seconds, 600);
    assert_eq!(client_id, "kobo:evt-1");
}

#[tokio::test]
async fn analytics_leave_content_rejects_a_pre_epoch_device_clock() {
    // A device clock stuck before 1970 combined with a large SecondsRead makes
    // `started_at = ended_at - seconds` negative. SessionReport::validate()
    // must catch this before the row reaches `reading_sessions` — a session
    // that skipped validation would silently corrupt future stats aggregates.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    let body = serde_json::json!({
        "Events": [{
            "Id": "evt-bad-clock",
            "EventType": "LeaveContent",
            "Timestamp": "1970-01-01T00:00:05Z",
            "Metrics": { "SecondsRead": 600 },
            "Attributes": { "volumeid": uuid }
        }]
    });
    let res = app
        .oneshot(post_json(format!("/kobo/{token}/v1/analytics/event"), body))
        .await
        .unwrap();

    // The batch contract still answers Success (per-event failures are
    // logged and skipped, never surfaced as a 4xx that makes the device
    // re-queue) — but no row must have been written.
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(res).await["Result"], "Success");

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reading_sessions WHERE user_id = ?")
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 0, "an invalid session must not be persisted");
}

#[tokio::test]
async fn analytics_replayed_batch_does_not_double_count_a_session() {
    // The event Id rides as the session client_id, so a device that never saw
    // the ack and re-posts the batch collapses onto the existing row (0052).
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    let body = serde_json::json!({
        "Events": [{
            "Id": "evt-1",
            "EventType": "LeaveContent",
            "Timestamp": "2026-07-26T12:00:00Z",
            "Metrics": { "SecondsRead": 600 },
            "Attributes": { "volumeid": uuid }
        }]
    });

    for _ in 0..2 {
        let res = app
            .clone()
            .oneshot(post_json(
                format!("/kobo/{token}/v1/analytics/event"),
                body.clone(),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    let rows: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM reading_sessions WHERE user_id = ?")
        .bind(uid)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(rows, 1);
}

#[tokio::test]
async fn analytics_rate_book_sets_and_clears_the_rating() {
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    let rate = serde_json::json!({
        "Events": [{
            "Id": "evt-2",
            "EventType": "RateBook",
            "Metrics": { "stars": 4 },
            "Attributes": { "volumeid": uuid }
        }]
    });
    app.clone()
        .oneshot(post_json(format!("/kobo/{token}/v1/analytics/event"), rate))
        .await
        .unwrap();
    let rec = db::ratings::get_rating(&pool, uid, &uuid).await.unwrap();
    assert_eq!(rec.unwrap().stars, 4.0);

    let clear = serde_json::json!({
        "Events": [{
            "Id": "evt-3",
            "EventType": "RateBook",
            "Metrics": { "stars": 0 },
            "Attributes": { "volumeid": uuid }
        }]
    });
    app.oneshot(post_json(
        format!("/kobo/{token}/v1/analytics/event"),
        clear,
    ))
    .await
    .unwrap();
    assert!(db::ratings::get_rating(&pool, uid, &uuid)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn analytics_unknown_event_types_are_acknowledged_and_dropped() {
    // A 4xx here would make the device re-queue and hammer the route, so even
    // a junk batch answers Success.
    let (app, _pool, token, _uid) = fixture().await;
    let body = serde_json::json!({
        "Events": [
            { "Id": "e1", "EventType": "OpenContent" },
            { "Id": "e2", "EventType": "LeaveContent" },
            { "Id": "e3", "EventType": "RateBook" }
        ]
    });

    let res = app
        .oneshot(post_json(format!("/kobo/{token}/v1/analytics/event"), body))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(res).await["Result"], "Success");
}

#[tokio::test]
async fn analytics_gettests_answers_the_initialization_pointer() {
    // The #926 resources map points `get_tests_request` at this route; a 404
    // there would be the server advertising a URL it doesn't serve.
    let (app, _pool, token, _uid) = fixture().await;
    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/analytics/gettests")))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(res).await["Result"], "Success");
}

#[tokio::test]
async fn analytics_gettests_answers_the_post_a_real_device_sends() {
    // Real firmware POSTs gettests even though it's a fetch; a 405 here aborts
    // the device's whole sync before `library/sync` is ever attempted.
    let (app, _pool, token, _uid) = fixture().await;
    let res = app
        .oneshot(post_json(
            format!("/kobo/{token}/v1/analytics/gettests"),
            serde_json::json!({}),
        ))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(body_json(res).await["Result"], "Success");
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
async fn analytics_rejects_an_invalid_token() {
    let (app, _pool, _token, _uid) = fixture().await;
    let res = app
        .oneshot(post_json(
            "/kobo/not-a-real-token/v1/analytics/event".to_owned(),
            serde_json::json!({ "Events": [] }),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn image_returns_304_when_the_if_none_match_etag_is_current() {
    // The 304 path fires before the cover bytes are ever loaded, so a current
    // validator answers bodyless even while the book has no stored cover.
    let (app, pool, token, _uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    let (id, lm): (i64, i64) = sqlx::query_as(
        "SELECT id, CAST(COALESCE(last_modified, 0) AS INTEGER) FROM books WHERE uuid = ?",
    )
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    let etag = format!("W/\"{id}-{lm}\"");

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/kobo/{token}/v1/books/{uuid}/thumbnail/400/600/100/false/image.jpg"
                ))
                .header("host", "omni.test")
                .header("if-none-match", &etag)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(res.headers().get("etag").unwrap().to_str().unwrap(), etag);
}

#[tokio::test]
async fn image_serves_the_body_when_the_etag_is_stale() {
    // A stale validator falls through to the normal serve path — here a 404,
    // since the fixture book has no stored cover. The point is that it did NOT
    // answer 304 against a stale tag.
    let (app, pool, token, _uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!(
                    "/kobo/{token}/v1/books/{uuid}/thumbnail/400/600/100/false/image.jpg"
                ))
                .header("host", "omni.test")
                .header("if-none-match", "W/\"stale\"")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn library_sync_advertises_the_source_epub_size() {
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    sqlx::query(
        "UPDATE book_files SET size_bytes = 123456
          WHERE book_id = (SELECT id FROM books WHERE uuid = ?)",
    )
    .bind(&uuid)
    .execute(&pool)
    .await
    .unwrap();
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    let json = body_json(res).await;
    let ent = &json.as_array().unwrap()[0]["NewEntitlement"];

    assert_eq!(ent["BookMetadata"]["DownloadUrls"][0]["Size"], 123456);
}

#[tokio::test]
async fn put_state_accepts_a_statistics_block() {
    // Statistics is parsed but deliberately unwritten (cumulative totals would
    // double-count against the LeaveContent sessions) — the contract here is
    // that a payload carrying it still round-trips as Success.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "solaris.epub", "Solaris", "Lem").await;
    let body = serde_json::json!({
        "ReadingStates": [{
            "StatusInfo": { "Status": "Reading" },
            "Statistics": { "SpentReadingMinutes": 42, "RemainingTimeMinutes": 90 }
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
    assert_eq!(body_json(res).await["RequestResult"], "Success");
    let rec = db::read_status::get_read_status(&pool, uid, &uuid)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.status, ReadStatus::Reading);
}

#[tokio::test]
async fn uuid_routes_reject_an_oversized_path_uuid() {
    let (app, _pool, token, _uid) = fixture().await;
    let oversized = "a".repeat(omnibus_shared::BOOK_UUID_MAX_LEN + 1);

    for uri in [
        format!("/kobo/{token}/v1/library/{oversized}/metadata"),
        format!("/kobo/{token}/v1/download/{oversized}"),
        format!("/kobo/{token}/v1/books/{oversized}/thumbnail/400/600/100/false/image.jpg"),
    ] {
        let res = app.clone().oneshot(get(uri.clone())).await.unwrap();
        assert_eq!(res.status(), StatusCode::BAD_REQUEST, "GET {uri}");
    }

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/kobo/{token}/v1/library/{oversized}/state"))
                .method("PUT")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({ "ReadingStates": [] }).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
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
async fn put_state_rejects_a_batch_exceeding_max_reading_states() {
    let (app, _pool, token, _uid) = fixture().await;
    let entries: Vec<Value> = (0..=dto::StateRequest::MAX_READING_STATES)
        .map(|_| serde_json::json!({}))
        .collect();
    let body = serde_json::json!({ "ReadingStates": entries });

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/kobo/{token}/v1/library/does-not-matter/state"))
                .method("PUT")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
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

/// #1391: kepubify is absent in the test environment, so `download` takes its
/// plain-EPUB fallback arm — which must still route through
/// `rewritten_or_source` (mirrors `api_get_ebook_download_bakes_metadata_override_into_epub`
/// in `ebooks/tests.rs`) rather than serving the raw on-disk file.
#[tokio::test]
async fn download_bakes_a_metadata_override_into_the_plain_epub_fallback() {
    use std::io::Cursor;

    let (app, pool, token, uid) = fixture().await;

    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("omnibus_kobo_dl_override_{pid}_{nanos}"));
    let export = tmp.join("export");
    std::fs::create_dir_all(&export).unwrap();
    // Isolate the export cache so the rewrite doesn't land in ./data.
    let _env =
        db::test_support::EnvVarGuard::set_os("OMNIBUS_EXPORT_EPUB_DIR", Some(export.as_os_str()));

    // Copy a real fixture EPUB so the rewrite has a valid container to parse.
    let fixture_epub = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../test_data/epubs/generated/alpha.epub");
    let stem = "alpha";
    std::fs::copy(&fixture_epub, tmp.join(format!("{stem}.epub"))).unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(tmp.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "55555555-5555-5555-5555-555555555555";
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, last_modified) \
         VALUES (?, ?, ?, 'Alpha', 1)",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(tmp.to_str().unwrap())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES ((SELECT id FROM books WHERE uuid = ?), 'EPUB', ?, 0)",
    )
    .bind(uuid)
    .bind(stem)
    .execute(&pool)
    .await
    .unwrap();

    let overrides = omnibus_shared::MetadataOverrides {
        title: Some("Stormlight #1".into()),
        ..Default::default()
    };
    db::upsert_metadata_overrides(&pool, uuid, &overrides, false, uid)
        .await
        .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/download/{uuid}")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();

    let doc = epub::doc::EpubDoc::from_reader(Cursor::new(bytes.to_vec()))
        .expect("downloaded bytes are a valid EPUB");
    assert_eq!(
        doc.mdata("title").map(|m| m.value.clone()),
        Some("Stormlight #1".to_string()),
        "kobo download fallback must carry the baked title override"
    );

    std::fs::remove_dir_all(&tmp).ok();
}

/// #1647 (AC2): a web-origin highlight created before the device ever
/// downloaded the book sits un-downsynced (`kobo_location` still `NULL` —
/// nothing had a kepub cache to derive against). Downloading the book must
/// materialize it and make the pair checkforchanges-reportable in the same
/// request, with no remove-and-re-download dance.
#[tokio::test]
async fn download_records_holding_the_book_and_materializes_a_pending_web_highlight() {
    let (app, pool, token, uid) = fixture().await;
    let (uuid, _guard, _lib) = seed_book_with_kepub_cache(&pool, "ac2", true).await;
    let device_id = db::kobo_devices::resolve_device_by_token(&pool, &token)
        .await
        .unwrap()
        .unwrap()
        .device_id;

    db::annotations::create_highlight(
        &pool,
        uid,
        &omnibus_shared::CreateHighlight {
            book_uuid: uuid.clone(),
            epub_cfi_range: "epubcfi(/6/2!/4/4,/1:0,/1:29)".into(),
            color: omnibus_shared::HighlightColor::Green,
            text: Some("Second paragraph target text.".into()),
            client_id: Some("web-ac2".into()),
        },
    )
    .await
    .unwrap();

    // Nothing to serve yet — the highlight has no `kobo_location`, and this
    // device hasn't downloaded the book.
    assert!(db::annotations::served_kobo_annotations(&pool, uid, &uuid)
        .await
        .unwrap()
        .is_empty());
    assert!(
        db::kobo::annotations::changed_book_uuids(&pool, uid, device_id)
            .await
            .unwrap()
            .is_empty()
    );

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/download/{uuid}")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let served = db::annotations::served_kobo_annotations(&pool, uid, &uuid)
        .await
        .unwrap();
    assert_eq!(served.len(), 1, "download materialized the pending row");
    assert_eq!(
        db::kobo::annotations::changed_book_uuids(&pool, uid, device_id)
            .await
            .unwrap(),
        vec![uuid],
        "the download-state row makes the pair reportable without a PATCH"
    );
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

#[tokio::test]
async fn put_state_persists_the_current_bookmark_position() {
    // #925: the device's percent + opaque KoboSpan location now land in
    // `reading_progress` instead of being logged and dropped.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    let body = serde_json::json!({
        "ReadingStates": [{
            "CurrentBookmark": {
                "ProgressPercent": 43,
                "Location": { "Source": "c1.xhtml", "Type": "KoboSpan", "Value": "kobo.9.1" }
            }
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
    let rec = db::progress::get_progress(&pool, uid, &uuid, omnibus_shared::ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.progress_percent, Some(43));
    assert_eq!(
        rec.epub_cfi, None,
        "no cached kepub for this book → no derived CFI, percent+span only"
    );
    let loc = rec.kobo_location.expect("location stored");
    assert!(loc.contains("kobo.9.1"), "got: {loc}");
}

#[tokio::test]
async fn put_state_batch_transaction_rolls_back_a_read_status_write_when_a_later_write_fails() {
    // Drives the `_tx` primitives directly, since a genuine mid-batch DB
    // error isn't reachable through a malformed HTTP body.
    let (_app, pool, _token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "hyperion.epub", "Hyperion", "Simmons").await;

    let mut tx = pool.begin().await.unwrap();
    db::read_status::set_read_status_tx(
        &mut tx,
        uid,
        &omnibus_shared::SetReadStatus {
            book_uuid: uuid.clone(),
            status: ReadStatus::Finished,
        },
    )
    .await
    .unwrap();

    let bad_update = omnibus_shared::ProgressUpdate {
        book_uuid: uuid.clone(),
        format: omnibus_shared::ProgressFormat::Audio,
        epub_cfi: Some("epubcfi(/6/4)".into()),
        audio_position_seconds: Some(10.0),
        progress_percent: None,
        kobo_location: None,
        book_file_id: None,
        client_updated_at: None,
    };
    let err = db::progress::upsert_progress_tx(&mut tx, uid, &bad_update)
        .await
        .expect_err("an audio row carrying an epub_cfi violates the reading_progress CHECK");
    assert!(matches!(err, db::progress::ProgressError::Sqlx(_)));
    drop(tx); // no `.commit()` — implicit rollback, matching `put_state`'s early return

    let status = db::read_status::get_read_status(&pool, uid, &uuid)
        .await
        .unwrap();
    assert!(
        status.is_none(),
        "the read-status write from the same batch must not survive the rollback"
    );
}

/// Source/kepub fixture pair used by the derivation tests: single-chapter
/// book where span kobo.2.1 starts the second paragraph.
const STATE_SOURCE_C1: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>C1</title></head>
<body><p>Opening paragraph text.</p><p>Second paragraph target text.</p></body>
</html>"#;

const STATE_KEPUB_C1: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<html xmlns="http://www.w3.org/1999/xhtml">
<head><title>C1</title></head>
<body><div id="book-columns"><div id="book-inner"><p><span class="koboSpan" id="kobo.1.1">Opening paragraph text.</span></p><p><span class="koboSpan" id="kobo.2.1">Second paragraph target text.</span></p></div></div></body>
</html>"#;

/// Seed a book whose EPUB really exists on disk and (optionally) whose
/// kepub cache is pre-populated, so span↔CFI derivation has both sides to
/// walk. Returns the book uuid. Caller holds the returned guards for the
/// test's lifetime.
async fn seed_book_with_kepub_cache(
    pool: &SqlitePool,
    tag: &str,
    with_kepub: bool,
) -> (
    String,
    crate::backend::test_support::DataDirGuard,
    std::path::PathBuf,
) {
    let guard = crate::backend::test_support::DataDirGuard::new(tag);
    let lib = db::test_support::make_test_dir(&format!("kobo_state_{tag}"));
    let (book_id, uuid) = db::test_support::seed_epub_book_at(pool, &lib).await;
    std::fs::write(
        lib.join("sub").join("book.epub"),
        db::test_support::build_test_epub(&[("c1.xhtml", STATE_SOURCE_C1)]),
    )
    .unwrap();
    if with_kepub {
        let kepub_dir = guard.path.join("kepub");
        std::fs::create_dir_all(&kepub_dir).unwrap();
        std::fs::write(
            kepub_dir.join(format!("{book_id}.kepub.epub")),
            db::test_support::build_test_kepub(&[("c1.xhtml", STATE_KEPUB_C1)]),
        )
        .unwrap();
    }
    (uuid, guard, lib)
}

fn state_put(token: &str, uuid: &str, body: serde_json::Value) -> Request<Body> {
    Request::builder()
        .uri(format!("/kobo/{token}/v1/library/{uuid}/state"))
        .method("PUT")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap()
}

#[tokio::test]
async fn put_state_derives_a_cfi_from_the_kobospan_when_the_kepub_is_cached() {
    let (app, pool, token, uid) = fixture().await;
    let (uuid, _guard, _lib) = seed_book_with_kepub_cache(&pool, "derive", true).await;
    let body = serde_json::json!({
        "ReadingStates": [{
            "CurrentBookmark": {
                "ProgressPercent": 43,
                "Location": { "Source": "c1.xhtml", "Type": "KoboSpan", "Value": "kobo.2.1" },
                "LastModified": "2026-01-02T03:04:05Z"
            }
        }]
    });

    let res = app.oneshot(state_put(&token, &uuid, body)).await.unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let rec = db::progress::get_progress(&pool, uid, &uuid, omnibus_shared::ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    // Second paragraph = body element child 2 → /4/4; its text node → /1:0.
    assert_eq!(rec.epub_cfi.as_deref(), Some("epubcfi(/6/2!/4/4/1:0)"));
    assert_eq!(
        rec.progress_percent,
        Some(43),
        "device percent kept verbatim"
    );
    assert!(rec.kobo_location.unwrap().contains("kobo.2.1"));
    // 2026-01-02T03:04:05Z — the device's own event time, not server-now.
    assert_eq!(rec.client_updated_at, 1_767_323_045);
}

#[tokio::test]
async fn get_state_returns_the_position_a_device_previously_put() {
    let (app, pool, token, _uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    let body = serde_json::json!({
        "ReadingStates": [{
            "CurrentBookmark": {
                "ProgressPercent": 43,
                "Location": { "Source": "c1.xhtml", "Type": "KoboSpan", "Value": "kobo.9.1" }
            }
        }]
    });
    let res = app
        .clone()
        .oneshot(state_put(&token, &uuid, body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/{uuid}/state")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let states = body_json(res).await;
    let state = &states.as_array().expect("array of one state")[0];
    assert_eq!(state["EntitlementId"], uuid.as_str());
    assert_eq!(state["CurrentBookmark"]["ProgressPercent"], 43);
    assert_eq!(state["CurrentBookmark"]["Location"]["Value"], "kobo.9.1");
    assert_eq!(state["StatusInfo"]["Status"], "ReadyToRead");
}

#[tokio::test]
async fn get_state_derives_a_kobospan_from_a_web_cfi_when_the_kepub_is_cached() {
    let (app, pool, token, uid) = fixture().await;
    let (uuid, _guard, _lib) = seed_book_with_kepub_cache(&pool, "get_state", true).await;
    // A web-reader position: CFI only, no Kobo anchor yet.
    let update = omnibus_shared::ProgressUpdate {
        book_uuid: uuid.clone(),
        format: omnibus_shared::ProgressFormat::Epub,
        epub_cfi: Some("epubcfi(/6/2!/4/4/1:0)".into()),
        audio_position_seconds: None,
        progress_percent: None,
        kobo_location: None,
        book_file_id: None,
        client_updated_at: None,
    };
    db::progress::upsert_progress(&pool, uid, &update)
        .await
        .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/{uuid}/state")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let states = body_json(res).await;
    let bookmark = &states.as_array().expect("array of one state")[0]["CurrentBookmark"];
    // The CFI points at the second paragraph → kobo.2.1 in the cached kepub.
    assert_eq!(bookmark["Location"]["Value"], "kobo.2.1");
    assert_eq!(bookmark["Location"]["Type"], "KoboSpan");
}

/// Pin a book's read-status and progress clocks to known, distinct instants.
/// `set_read_status` / `upsert_progress` stamp server-now, which no assertion
/// on an exact wire timestamp can name.
async fn pin_state_clocks(
    pool: &SqlitePool,
    user_id: i64,
    uuid: &str,
    status_at: i64,
    progress_at: i64,
) {
    db::read_status::set_read_status(
        pool,
        user_id,
        &omnibus_shared::SetReadStatus {
            book_uuid: uuid.to_owned(),
            status: ReadStatus::Reading,
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE book_read_status SET updated_at = ? WHERE user_id = ? AND book_uuid = ?")
        .bind(status_at)
        .bind(user_id)
        .bind(uuid)
        .execute(pool)
        .await
        .unwrap();
    db::progress::upsert_progress(
        pool,
        user_id,
        &omnibus_shared::ProgressUpdate {
            book_uuid: uuid.to_owned(),
            format: omnibus_shared::ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/2!/4/4/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: Some(58),
            // Already-enriched row: a present anchor keeps the sync-out span
            // derivation (and its clock-neutral write-back) out of the way.
            kobo_location: Some(
                r#"{"Source":"c1.xhtml","Type":"KoboSpan","Value":"kobo.2.1"}"#.into(),
            ),
            book_file_id: None,
            client_updated_at: Some(progress_at),
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn get_state_stamps_the_bookmark_clock_so_the_device_can_adopt_a_web_position() {
    // The regression this exists for: the device arbitrates the served
    // bookmark against its own by `CurrentBookmark.LastModified`, so an
    // unstamped one always loses and a web-side position never lands.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    pin_state_clocks(&pool, uid, &uuid, 1_700_000_000, 1_700_001_000).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/{uuid}/state")))
        .await
        .unwrap();
    let states = body_json(res).await;
    let state = &states.as_array().expect("array of one state")[0];

    // 1_700_001_000 → the progress row's own event time, not server-now.
    assert_eq!(
        state["CurrentBookmark"]["LastModified"],
        "2023-11-14T22:30:00Z"
    );
}

#[tokio::test]
async fn get_state_stamps_the_status_clock_independently_of_the_bookmark_clock() {
    // Each sub-object is reconciled against its own device-side clock, so
    // the MAXed envelope value must not stand in for either: reporting it as
    // the status clock would claim the status moved every time only the
    // position did.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    pin_state_clocks(&pool, uid, &uuid, 1_700_000_000, 1_700_001_000).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/{uuid}/state")))
        .await
        .unwrap();
    let states = body_json(res).await;
    let state = &states.as_array().expect("array of one state")[0];

    assert_eq!(state["StatusInfo"]["LastModified"], "2023-11-14T22:13:20Z");
    assert_eq!(
        state["CurrentBookmark"]["LastModified"],
        "2023-11-14T22:30:00Z"
    );
    // The envelope carries the newer of the two, and PriorityTimestamp
    // mirrors it.
    assert_eq!(state["LastModified"], "2023-11-14T22:30:00Z");
    assert_eq!(state["PriorityTimestamp"], "2023-11-14T22:30:00Z");
}

#[tokio::test]
async fn get_state_leaves_an_untouched_books_clocks_unstamped() {
    // A book with no state must not carry an invented bookmark time: an
    // empty bookmark has to lose arbitration, not win it at the epoch.
    let (app, pool, token, _uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/{uuid}/state")))
        .await
        .unwrap();
    let states = body_json(res).await;
    let state = &states.as_array().expect("array of one state")[0];

    assert!(state["StatusInfo"]["LastModified"].is_null());
    assert!(state["CurrentBookmark"]["LastModified"].is_null());
    assert_eq!(state["StatusInfo"]["Status"], "ReadyToRead");
}

#[tokio::test]
async fn library_sync_stamps_the_reading_state_clocks_on_a_new_entitlement() {
    // Same payload, second delivery path — the device adopts position from
    // either, so both must carry the arbitration clocks.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    pin_state_clocks(&pool, uid, &uuid, 1_700_000_000, 1_700_001_000).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    let json = body_json(res).await;
    let rs = &json.as_array().unwrap()[0]["NewEntitlement"]["ReadingState"];

    assert_eq!(
        rs["CurrentBookmark"]["LastModified"],
        "2023-11-14T22:30:00Z"
    );
    assert_eq!(rs["StatusInfo"]["LastModified"], "2023-11-14T22:13:20Z");
    assert_eq!(rs["PriorityTimestamp"], "2023-11-14T22:30:00Z");
}

#[tokio::test]
async fn put_state_still_accepts_a_status_info_without_a_last_modified() {
    // `StatusInfo` is shared between the sync-out response and the inbound
    // PUT; the field added for the former must stay optional for the latter,
    // which real firmware does not always send.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    let body = serde_json::json!({
        "ReadingStates": [{ "StatusInfo": { "Status": "Finished" } }]
    });

    let res = app.oneshot(state_put(&token, &uuid, body)).await.unwrap();

    assert_eq!(res.status(), StatusCode::OK);
    let stored = db::read_status::get_read_status(&pool, uid, &uuid)
        .await
        .unwrap();
    assert_eq!(stored.map(|s| s.status), Some(ReadStatus::Finished));
}

#[tokio::test]
async fn get_state_returns_404_for_an_unknown_book() {
    let (app, _pool, token, _uid) = fixture().await;
    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/no-such-uuid/state")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn put_state_rejects_a_bookmark_older_than_the_stored_position() {
    // A test device (or a long-offline Kobo) replaying an old position must
    // not clobber a newer one — the device's LastModified drives the guard.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    db::progress::upsert_progress(
        &pool,
        uid,
        &omnibus_shared::ProgressUpdate {
            book_uuid: uuid.clone(),
            format: omnibus_shared::ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/2!/4/8/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(1_900_000_000),
        },
    )
    .await
    .unwrap();

    let body = serde_json::json!({
        "ReadingStates": [{
            "CurrentBookmark": {
                "ProgressPercent": 1,
                "Location": { "Source": "c1.xhtml", "Type": "KoboSpan", "Value": "kobo.1.1" },
                "LastModified": "2020-01-01T00:00:00Z"
            }
        }]
    });
    let res = app.oneshot(state_put(&token, &uuid, body)).await.unwrap();

    assert_eq!(res.status(), StatusCode::OK, "a stale write still ACKs");
    let rec = db::progress::get_progress(&pool, uid, &uuid, omnibus_shared::ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        rec.epub_cfi.as_deref(),
        Some("epubcfi(/6/2!/4/8/1:0)"),
        "the newer web position must survive a stale device replay"
    );
    assert_eq!(rec.progress_percent, None);
}

#[tokio::test]
async fn put_state_prefers_the_bookmark_timestamp_over_entry_level_ones() {
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    let body = serde_json::json!({
        "ReadingStates": [{
            "LastModified": "2026-01-01T00:00:00Z",
            "PriorityTimestamp": "2026-01-01T00:00:01Z",
            "CurrentBookmark": {
                "ProgressPercent": 10,
                "LastModified": "2026-01-02T00:00:00Z"
            }
        }]
    });
    let res = app.oneshot(state_put(&token, &uuid, body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let rec = db::progress::get_progress(&pool, uid, &uuid, omnibus_shared::ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    // 2026-01-02T00:00:00Z, from the bookmark, not the entry.
    assert_eq!(rec.client_updated_at, 1_767_312_000);
}

#[tokio::test]
async fn put_state_falls_back_to_server_now_for_garbage_timestamps() {
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    let before = time::OffsetDateTime::now_utc().unix_timestamp();
    let body = serde_json::json!({
        "ReadingStates": [{
            "CurrentBookmark": {
                "ProgressPercent": 10,
                "LastModified": "not a timestamp"
            }
        }]
    });
    let res = app.oneshot(state_put(&token, &uuid, body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let rec = db::progress::get_progress(&pool, uid, &uuid, omnibus_shared::ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert!(
        rec.client_updated_at >= before,
        "unparseable device time must fall back to receipt time"
    );
}

/// Pull the first `ReadingState` object out of a `library/sync` body.
fn first_reading_state(json: &Value) -> Option<Value> {
    json.as_array()?.iter().find_map(|item| {
        item.get("NewEntitlement")
            .or_else(|| item.get("ChangedReadingState"))
            .and_then(|e| e.get("ReadingState"))
            .cloned()
    })
}

#[tokio::test]
async fn library_sync_derives_a_kobospan_from_a_web_written_cfi() {
    let (app, pool, token, uid) = fixture().await;
    let (uuid, _guard, _lib) = seed_book_with_kepub_cache(&pool, "syncout", true).await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    // A web-reader write: CFI only, percent/span cleared by replace-all.
    db::progress::upsert_progress(
        &pool,
        uid,
        &omnibus_shared::ProgressUpdate {
            book_uuid: uuid.clone(),
            format: omnibus_shared::ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/2!/4/4/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(100),
        },
    )
    .await
    .unwrap();

    let res = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    let reading_state = first_reading_state(&json).expect("a reading state in the sync body");

    let bookmark = &reading_state["CurrentBookmark"];
    assert_eq!(
        bookmark["Location"]["Value"], "kobo.2.1",
        "the CFI at the second paragraph maps to its span: {bookmark}"
    );
    assert_eq!(bookmark["Location"]["Source"], "c1.xhtml");
    assert_eq!(bookmark["Location"]["Type"], "KoboSpan");
    // Anchor at char 24 of 53 visible chars → 45%.
    assert_eq!(bookmark["ProgressPercent"], 44);
    assert_eq!(
        reading_state["LastModified"], "1970-01-01T00:01:40Z",
        "the reading state must carry the position's own event time (100)"
    );

    // Write-back: the derived span is persisted clock-neutrally…
    let rec = db::progress::get_progress(&pool, uid, &uuid, omnibus_shared::ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert!(rec.kobo_location.unwrap().contains("kobo.2.1"));
    assert_eq!(rec.progress_percent, Some(44));
    assert_eq!(
        rec.client_updated_at, 100,
        "write-back must not bump the clock"
    );

    // …so a second sync has nothing new to announce.
    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    let json = body_json(res).await;
    assert!(
        json.as_array().unwrap().is_empty(),
        "derivation write-back must not re-fire the sync delta: {json}"
    );
}

#[tokio::test]
async fn library_sync_emits_percent_only_when_the_kepub_is_absent() {
    let (app, pool, token, uid) = fixture().await;
    let (uuid, _guard, _lib) = seed_book_with_kepub_cache(&pool, "nokepub", false).await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    db::progress::upsert_progress(
        &pool,
        uid,
        &omnibus_shared::ProgressUpdate {
            book_uuid: uuid.clone(),
            format: omnibus_shared::ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/2!/4/4/1:0)".into()),
            audio_position_seconds: None,
            progress_percent: None,
            kobo_location: None,
            book_file_id: None,
            client_updated_at: Some(100),
        },
    )
    .await
    .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let json = body_json(res).await;
    let reading_state = first_reading_state(&json).expect("a reading state in the sync body");
    let bookmark = &reading_state["CurrentBookmark"];
    assert_eq!(
        bookmark["ProgressPercent"], 44,
        "the percent half needs only the source EPUB: {bookmark}"
    );
    assert!(
        bookmark.get("Location").is_none(),
        "no kepub → no span to invent: {bookmark}"
    );
    // Not persisted: the row still means "no span known" so a later sync
    // (with the queued conversion done) retries the full derivation.
    let rec = db::progress::get_progress(&pool, uid, &uuid, omnibus_shared::ProgressFormat::Epub)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(rec.kobo_location, None);
}

#[tokio::test]
async fn put_state_ignores_a_bookmark_with_no_position() {
    // The device sends the field either way; an empty one is a no-op, not a
    // validation failure that would 500 the sync.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    let body = serde_json::json!({
        "ReadingStates": [{ "CurrentBookmark": {} }]
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
    assert!(
        db::progress::get_progress(&pool, uid, &uuid, omnibus_shared::ProgressFormat::Epub)
            .await
            .unwrap()
            .is_none(),
        "an empty bookmark must not create a row"
    );
}

#[tokio::test]
async fn library_sync_reports_real_read_status_and_position() {
    // AC1/AC2: a book finished and positioned on another surface syncs to a
    // fresh device carrying that status and percent, not the hardcoded default.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    db::read_status::set_read_status(
        &pool,
        uid,
        &omnibus_shared::SetReadStatus {
            book_uuid: uuid.clone(),
            status: omnibus_shared::ReadStatus::Finished,
        },
    )
    .await
    .unwrap();
    db::progress::upsert_progress(
        &pool,
        uid,
        &omnibus_shared::ProgressUpdate {
            book_uuid: uuid.clone(),
            format: omnibus_shared::ProgressFormat::Epub,
            epub_cfi: None,
            audio_position_seconds: None,
            progress_percent: Some(88),
            kobo_location: Some(r#"{"Value":"kobo.12.4"}"#.into()),
            client_updated_at: None,
            book_file_id: None,
        },
    )
    .await
    .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    let json = body_json(res).await;
    let rs = &json.as_array().unwrap()[0]["NewEntitlement"]["ReadingState"];
    assert_eq!(rs["StatusInfo"]["Status"], "Finished");
    assert_eq!(rs["CurrentBookmark"]["ProgressPercent"], 88);
    assert_eq!(rs["CurrentBookmark"]["Location"]["Value"], "kobo.12.4");
}

#[tokio::test]
async fn library_sync_reports_ready_to_read_for_an_untouched_book() {
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    let json = body_json(res).await;
    let rs = &json.as_array().unwrap()[0]["NewEntitlement"]["ReadingState"];
    assert_eq!(rs["StatusInfo"]["Status"], "ReadyToRead");
    // `CurrentBookmark` serializes its fields only when set, so an untouched
    // book emits an empty object rather than invented zeroes.
    assert!(rs["CurrentBookmark"]["ProgressPercent"].is_null());
}

#[tokio::test]
async fn library_sync_re_announces_a_status_change_to_a_device_that_holds_the_book() {
    // The state-only delta: metadata never moved, so without it a book
    // finished on the web would never reach a device that already has it.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    let first = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(body_json(first).await.as_array().unwrap().len(), 1);

    // Age the snapshot past `synced_at`'s 1-second granularity.
    sqlx::query("UPDATE kobo_books_sync SET synced_at = 1")
        .execute(&pool)
        .await
        .unwrap();
    db::read_status::set_read_status(
        &pool,
        uid,
        &omnibus_shared::SetReadStatus {
            book_uuid: uuid.clone(),
            status: omnibus_shared::ReadStatus::Finished,
        },
    )
    .await
    .unwrap();

    let second = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    let json = body_json(second).await;
    let arr = json.as_array().unwrap();
    assert_eq!(arr.len(), 1, "a state change emits one bare item");
    assert_eq!(
        arr[0]["ChangedReadingState"]["ReadingState"]["StatusInfo"]["Status"],
        "Finished"
    );

    // And once delivered, the device is current again.
    let third = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert!(body_json(third).await.as_array().unwrap().is_empty());
}

// --- 500-on-DB-failure coverage (#1392) ---
//
// `KoboAuthUser` authenticates via `kobo_devices`, so dropping `books`
// leaves auth intact and forces the failure inside each handler's own
// `internal(...)` call site rather than at the extractor.

#[tokio::test]
async fn library_sync_returns_500_on_db_failure() {
    // `sync_books` short-circuits to an empty `Ok` when the user has no
    // opted-in shelf, never touching `books` — so a real opted-in book is
    // required to actually reach (and fail) that query.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    opt_in(&pool, uid, std::slice::from_ref(&uuid)).await;
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn metadata_returns_500_on_db_failure() {
    let (app, pool, token, _uid) = fixture().await;
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/any-uuid/metadata")))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn download_returns_500_on_db_failure_when_resolving_book_id_fails() {
    let (app, pool, token, _uid) = fixture().await;
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/download/any-uuid")))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn download_returns_500_on_db_failure_when_locating_the_epub_file_fails() {
    // Force kepubify absent so `download` deterministically takes the
    // plain-EPUB fallback and calls `book_file_path`, regardless of whether
    // kepubify happens to be installed in the environment running this test.
    let _kepubify_absent =
        db::test_support::EnvVarGuard::set("OMNIBUS_KEPUBIFY_PATH", Some("/no/such/kepubify"));
    // Keep `books` intact (the uuid must resolve) and drop `book_files`
    // instead, so this reaches that second `internal(...)` call site
    // rather than the earlier `resolve_book_id_by_uuid` one.
    let (app, pool, token, _uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "solaris.epub", "Solaris", "Lem").await;
    sqlx::query("DROP TABLE book_files")
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/download/{uuid}")))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn put_state_returns_500_on_db_failure() {
    let (app, pool, token, _uid) = fixture().await;
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();
    let body = serde_json::json!({
        "ReadingStates": [{
            "StatusInfo": { "Status": "Finished" }
        }]
    });

    let res = app
        .oneshot(
            Request::builder()
                .uri(format!("/kobo/{token}/v1/library/any-uuid/state"))
                .method("PUT")
                .header("content-type", "application/json")
                .body(Body::from(body.to_string()))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn image_returns_500_on_db_failure() {
    let (app, pool, token, _uid) = fixture().await;
    sqlx::query("DROP TABLE books")
        .execute(&pool)
        .await
        .unwrap();

    let res = app
        .oneshot(get(format!(
            "/kobo/{token}/v1/books/any-uuid/thumbnail/400/600/100/false/image.jpg"
        )))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
