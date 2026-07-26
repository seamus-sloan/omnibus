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
    // 150 books > SYNC_PAGE_SIZE (100), so this exercises the real protocol
    // loop: page + `x-kobo-sync: continue` → device re-hits → final page with
    // no header. Unlike Calibre-Web's SYNC_ITEM_LIMIT, nothing is dropped —
    // the page bounds the response, not the sync.
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
