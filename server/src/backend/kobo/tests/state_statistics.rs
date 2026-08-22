//! The `Statistics` block a device attaches to a reading state: when it is
//! stored, how it round-trips, and the clock it is dated with.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use omnibus_shared::ReadStatus;
use sqlx::Row;
use tower::ServiceExt;

use super::{body_json, fixture, get, pin_state_clocks, state_put};

#[tokio::test]
async fn put_state_accepts_a_statistics_block_but_stores_none_without_a_position() {
    // Statistics annotate a position; they don't create one (the row CHECK
    // requires a CFI or a percent, and a position-less row would surface on
    // the Continue-reading rail at 0%). This payload carries no bookmark, so
    // the block is dropped — and the PUT still round-trips as Success.
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
    assert!(
        db::progress::get_progress(&pool, uid, &uuid, omnibus_shared::ProgressFormat::Epub)
            .await
            .unwrap()
            .is_none(),
        "statistics must not conjure a position row"
    );
}

#[tokio::test]
async fn put_state_persists_a_statistics_block_alongside_the_bookmark() {
    // AC1: the block lands on the position row the same entry creates.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "solaris.epub", "Solaris", "Lem").await;
    let body = serde_json::json!({
        "ReadingStates": [{
            "CurrentBookmark": { "ProgressPercent": 43 },
            "Statistics": {
                "SpentReadingMinutes": 340,
                "RemainingTimeMinutes": 75,
                "LastModified": "2023-11-14T22:45:00Z"
            }
        }]
    });

    let res = app.oneshot(state_put(&token, &uuid, body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let row = sqlx::query(
        "SELECT kobo_spent_reading_minutes, kobo_remaining_time_minutes,
                kobo_statistics_updated_at
           FROM reading_progress WHERE user_id = ? AND book_uuid = ? AND format = 'epub'",
    )
    .bind(uid)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<Option<i64>, _>(0), Some(340));
    assert_eq!(row.get::<Option<i64>, _>(1), Some(75));
    // The device's own clock, not server receipt time.
    assert_eq!(row.get::<Option<i64>, _>(2), Some(1_700_001_900));
}

#[tokio::test]
async fn put_state_drops_a_negative_statistics_counter_rather_than_clamping_it() {
    // A value we can't interpret has no safe number to clamp toward — these
    // are only ever echoed back. The row CHECK is the backstop, and letting it
    // fire would abort the whole batch's transaction over one bad field.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "solaris.epub", "Solaris", "Lem").await;
    let body = serde_json::json!({
        "ReadingStates": [{
            "CurrentBookmark": { "ProgressPercent": 43 },
            "Statistics": { "SpentReadingMinutes": -5, "RemainingTimeMinutes": 75 }
        }]
    });

    let res = app.oneshot(state_put(&token, &uuid, body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let row = sqlx::query(
        "SELECT kobo_spent_reading_minutes, kobo_remaining_time_minutes
           FROM reading_progress WHERE user_id = ? AND book_uuid = ? AND format = 'epub'",
    )
    .bind(uid)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<Option<i64>, _>(0), None, "negative dropped");
    assert_eq!(row.get::<Option<i64>, _>(1), Some(75), "sibling kept");
}

#[tokio::test]
async fn get_state_round_trips_a_statistics_block_a_device_previously_put() {
    // AC5: PUT then GET returns the same counters.
    let (app, pool, token, _uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    let body = serde_json::json!({
        "ReadingStates": [{
            "CurrentBookmark": { "ProgressPercent": 43 },
            "Statistics": { "SpentReadingMinutes": 340, "RemainingTimeMinutes": 75 }
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
    assert_eq!(state["Statistics"]["SpentReadingMinutes"], 340);
    assert_eq!(state["Statistics"]["RemainingTimeMinutes"], 75);
}

#[tokio::test]
async fn get_state_omits_statistics_for_a_book_with_none_stored() {
    // AC3: an absent block is today's behaviour and strictly safer than a
    // zeroed one — the device applies newest-wins, so zeroes stamped `now`
    // would overwrite its real totals.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    pin_state_clocks(&pool, uid, &uuid, 1_700_000_000, 1_700_001_000).await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/{uuid}/state")))
        .await
        .unwrap();
    let states = body_json(res).await;
    let state = &states.as_array().expect("array of one state")[0];
    assert_eq!(state["CurrentBookmark"]["ProgressPercent"], 58);
    assert!(
        state.get("Statistics").is_none(),
        "no stored block → no field at all, got: {state}"
    );
}

#[tokio::test]
async fn get_state_emits_the_devices_statistics_clock_and_never_server_now() {
    // AC4, and the regression the whole feature is shaped around: the device
    // arbitrates Statistics by newest-timestamp-wins (#1652). A block stamped
    // `now` would win against the device's own and wipe its reading totals —
    // trading a stuck bookmark for lost stats, which is the worse failure.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    pin_state_clocks(&pool, uid, &uuid, 1_700_000_000, 1_700_001_000).await;
    let body = serde_json::json!({
        "ReadingStates": [{
            "Statistics": {
                "SpentReadingMinutes": 340,
                "RemainingTimeMinutes": 75,
                "LastModified": "2023-11-14T22:45:00Z"
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
    let states = body_json(res).await;
    let state = &states.as_array().expect("array of one state")[0];
    // The device's own stamp, echoed verbatim. Server-now would serialize to
    // the current year, so this exact-match is the guard.
    assert_eq!(state["Statistics"]["LastModified"], "2023-11-14T22:45:00Z");
    assert_eq!(state["Statistics"]["SpentReadingMinutes"], 340);
}

#[tokio::test]
async fn put_state_does_not_date_statistics_with_the_entry_level_clock() {
    // The bookmark falls back to the entry clock; statistics must not. An
    // entry stamp is a device clock, but it would date counters the device
    // didn't date — and this stamp is what decides whether the device
    // overwrites its own totals with our echo.
    let (app, pool, token, uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    let body = serde_json::json!({
        "ReadingStates": [{
            "LastModified": "2023-11-14T22:45:00Z",
            "PriorityTimestamp": "2023-11-14T22:45:00Z",
            "CurrentBookmark": { "ProgressPercent": 43 },
            "Statistics": { "SpentReadingMinutes": 340 }
        }]
    });

    let res = app.oneshot(state_put(&token, &uuid, body)).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);

    let row = sqlx::query(
        "SELECT kobo_spent_reading_minutes, kobo_statistics_updated_at
           FROM reading_progress WHERE user_id = ? AND book_uuid = ? AND format = 'epub'",
    )
    .bind(uid)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(row.get::<Option<i64>, _>(0), Some(340), "counter stored");
    assert_eq!(
        row.get::<Option<i64>, _>(1),
        None,
        "the entry clock must not stand in for Statistics.LastModified"
    );
}

#[tokio::test]
async fn get_state_omits_the_statistics_clock_when_the_device_sent_none() {
    // An unstamped block loses the device's arbitration and is dropped, which
    // is the safe direction: inventing a clock is what would overwrite its
    // totals. So the counters ride along unstamped rather than freshly dated.
    let (app, pool, token, _uid) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    let body = serde_json::json!({
        "ReadingStates": [{
            "CurrentBookmark": { "ProgressPercent": 43 },
            "Statistics": { "SpentReadingMinutes": 340 }
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
    let states = body_json(res).await;
    let stats = &states.as_array().expect("array of one state")[0]["Statistics"];
    assert_eq!(stats["SpentReadingMinutes"], 340);
    assert!(
        stats.get("LastModified").is_none(),
        "no device clock → unstamped, got: {stats}"
    );
}
