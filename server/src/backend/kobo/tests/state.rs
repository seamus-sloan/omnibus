//! `/v1/library/{uuid}/state`: the bookmark position and read status a
//! device pushes and reads back, the CFI ↔ kobospan translation, the clocks
//! that let a device adopt a web-written position, and the batch limits.

use axum::{
    body::Body,
    http::{Request, StatusCode},
};
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use omnibus_shared::ReadStatus;
use serde_json::Value;
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;

use super::super::*;
use super::{body_json, fixture, get, pin_state_clocks, seed_book_with_kepub_cache, state_put};

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
