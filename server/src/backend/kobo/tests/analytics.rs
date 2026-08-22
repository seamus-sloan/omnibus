//! `/v1/analytics/event` and `/v1/analytics/gettests`: the reading sessions,
//! ratings, and unknown event types a device reports after it closes a book.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use serde_json::Value;
use tower::ServiceExt;

use super::super::*;
use super::{body_json, fixture, get};

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
async fn analytics_event_rejects_a_batch_exceeding_max_analytics_events() {
    let (app, _pool, token, _uid) = fixture().await;
    let events: Vec<Value> = (0..=analytics::MAX_ANALYTICS_EVENTS)
        .map(|_| serde_json::json!({ "Id": "e", "EventType": "OpenContent" }))
        .collect();
    let body = serde_json::json!({ "Events": events });

    let res = app
        .oneshot(post_json(format!("/kobo/{token}/v1/analytics/event"), body))
        .await
        .unwrap();

    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
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
