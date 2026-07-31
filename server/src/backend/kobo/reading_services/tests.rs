//! HTTP-layer contract tests for the Reading Services annotation channel,
//! driving `reading_services_router` via `oneshot` against an in-memory DB
//! (#928 pattern). The device flow — PATCH upload → checkforchanges → GET →
//! reconcile-by-omission — is replayed at the HTTP layer, including the AC5
//! first-sync guard for pre-wireless backlogs.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    response::Response,
    Router,
};
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;

const HW_ID: &str = "kobo-hw-0001";

/// Reading Services router on a fresh in-memory DB, plus the owning user's id
/// and a registered device whose hardware id is already learned — the state a
/// real device is in after its first tokened `/kobo/{token}/v1` call.
async fn fixture() -> (Router, SqlitePool, i64, i64) {
    let pool = db::init_db("sqlite::memory:").await.unwrap();
    let app = reading_services_router(AppState::new(pool.clone()));
    let user = auth_test_support::create_user(&pool, "kobo-reader").await;
    let device = db::kobo_devices::create_device(&pool, user.id, "Test Kobo")
        .await
        .unwrap();
    db::kobo_devices::learn_kobo_device_id(&pool, device.id, HW_ID)
        .await
        .unwrap();
    (app, pool, user.id, device.id)
}

async fn body_json(res: Response) -> Value {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

fn request(method: &str, uri: &str, hw: Option<&str>, body: Option<&Value>) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(hw) = hw {
        builder = builder.header("x-kobo-deviceid", hw);
    }
    match body {
        Some(v) => builder
            .header("content-type", "application/json")
            .body(Body::from(serde_json::to_vec(v).unwrap()))
            .unwrap(),
        None => builder.body(Body::empty()).unwrap(),
    }
}

fn annotations_uri(content_id: &str) -> String {
    format!("/api/v3/content/{content_id}/annotations")
}

fn upload_body(id: &str, color: &str, note: Option<&str>) -> Value {
    let mut annotation = json!({
        "id": id,
        "type": if note.is_some() { "note" } else { "highlight" },
        "highlightColor": color,
        "highlightedText": "the highlighted passage",
        "clientLastModifiedUtc": "2026-07-26T10:00:00Z",
        "location": {
            "span": {
                "chapterFilename": "OEBPS/ch1.xhtml",
                "startPath": "span#kobo\\.1\\.2",
                "startChar": 3,
                "endPath": "span#kobo\\.1\\.4",
                "endChar": 9
            }
        }
    });
    if let Some(note) = note {
        annotation["noteText"] = json!(note);
    }
    json!({ "updatedAnnotations": [annotation] })
}

/// PATCH one annotation and drain the GET so the pair is adopted and acked.
async fn upload_and_ack(app: &Router, uuid: &str, id: &str, color: &str, note: Option<&str>) {
    let res = app
        .clone()
        .oneshot(request(
            "PATCH",
            &annotations_uri(uuid),
            Some(HW_ID),
            Some(&upload_body(id, color, note)),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    let res = app
        .clone()
        .oneshot(request("GET", &annotations_uri(uuid), Some(HW_ID), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let _ = body_json(res).await; // draining the body is what acks
}

async fn check_for_changes(app: &Router) -> Vec<String> {
    let res = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v3/content/checkforchanges",
            Some(HW_ID),
            Some(&json!([])),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    serde_json::from_value(body_json(res).await).unwrap()
}

#[tokio::test]
async fn get_annotations_rejects_an_unknown_hardware_device_id() {
    let (app, pool, _user, _device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    let res = app
        .clone()
        .oneshot(request(
            "GET",
            &annotations_uri(&uuid),
            Some("hw-never-learned"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);

    let res = app
        .oneshot(request("GET", &annotations_uri(&uuid), None, None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "missing header too");
}

#[tokio::test]
async fn get_annotations_returns_404_for_an_unknown_book() {
    let (app, _pool, _user, _device) = fixture().await;
    let res = app
        .oneshot(request(
            "GET",
            &annotations_uri("no-such-uuid"),
            Some(HW_ID),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn get_annotations_rejects_an_oversized_content_id_with_400() {
    let (app, _pool, _user, _device) = fixture().await;
    let oversized = "u".repeat(omnibus_shared::BOOK_UUID_MAX_LEN + 1);
    let res = app
        .oneshot(request(
            "GET",
            &annotations_uri(&oversized),
            Some(HW_ID),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn get_annotations_answers_not_modified_for_an_unadopted_pair() {
    let (app, pool, _user, _device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    // AC5: no server-side annotations and no prior PATCH — an empty 200 here
    // would erase the device's pre-wireless highlights by omission.
    let res = app
        .oneshot(request("GET", &annotations_uri(&uuid), Some(HW_ID), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
    assert_eq!(res.headers()["etag"], "W/\"0\"");
}

#[tokio::test]
async fn patch_annotations_rejects_an_oversized_content_id_with_400() {
    let (app, _pool, _user, _device) = fixture().await;
    let oversized = "u".repeat(omnibus_shared::BOOK_UUID_MAX_LEN + 1);
    let res = app
        .oneshot(request(
            "PATCH",
            &annotations_uri(&oversized),
            Some(HW_ID),
            Some(&upload_body("kobo-ann-1", "yellow", None)),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn patch_then_get_round_trips_a_device_highlight_with_a_weak_etag() {
    let (app, pool, _user, _device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    let res = app
        .clone()
        .oneshot(request(
            "PATCH",
            &annotations_uri(&uuid),
            Some(HW_ID),
            Some(&upload_body("kobo-ann-1", "yellow", Some("margin note"))),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let res = app
        .oneshot(request("GET", &annotations_uri(&uuid), Some(HW_ID), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let etag = res.headers()["etag"].to_str().unwrap().to_owned();
    assert!(etag.starts_with("W/\""), "weak ETag, got {etag}");
    assert_ne!(etag, "W/\"0\"");

    let body = body_json(res).await;
    assert!(body["nextPageOffsetToken"].is_null());
    let annotations = body["annotations"].as_array().unwrap();
    assert_eq!(annotations.len(), 1);
    let a = &annotations[0];
    assert_eq!(a["id"], "kobo-ann-1");
    assert_eq!(a["type"], "note");
    assert_eq!(a["highlightColor"], "yellow");
    assert_eq!(a["highlightedText"], "the highlighted passage");
    assert_eq!(a["noteText"], "margin note");
    assert!(a["context"].is_null());
    assert_eq!(a["attachments"], json!({}));
    // The location object comes back verbatim — the device's anchor is
    // opaque to the server.
    assert_eq!(a["location"]["span"]["startPath"], "span#kobo\\.1\\.2");
    assert_eq!(a["location"]["span"]["endChar"], 9);
    // camelCase keys are the wire contract.
    assert!(a.get("clientLastModifiedUtc").is_some());
}

#[tokio::test]
async fn patch_replay_of_the_same_body_creates_no_duplicate_rows() {
    let (app, pool, user, _device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    let body = upload_body("kobo-ann-1", "blue", None);

    for _ in 0..2 {
        let res = app
            .clone()
            .oneshot(request(
                "PATCH",
                &annotations_uri(&uuid),
                Some(HW_ID),
                Some(&body),
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
    }

    let listed = db::annotations::list_highlights(&pool, user, &uuid)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].color, omnibus_shared::HighlightColor::Blue);
}

#[tokio::test]
async fn patch_tolerates_garbage_and_variant_delete_shapes_without_500() {
    let (app, pool, user, _device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    upload_and_ack(&app, &uuid, "kobo-keep", "green", None).await;
    upload_and_ack(&app, &uuid, "kobo-drop", "yellow", None).await;

    // A mixed bag: a delete as an {id} object, an entry with a deleted flag,
    // an idless entry (skipped), and a non-object entry (skipped).
    let body = json!({
        "updatedAnnotations": [
            { "highlightColor": "green", "location": {} },
            { "id": "kobo-flagged", "deleted": true, "location": {} },
            42
        ],
        "deletedAnnotations": [ { "id": "kobo-drop" } ]
    });
    let res = app
        .clone()
        .oneshot(request(
            "PATCH",
            &annotations_uri(&uuid),
            Some(HW_ID),
            Some(&body),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let listed = db::annotations::list_highlights(&pool, user, &uuid)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1, "kobo-drop deleted, garbage skipped");
    assert_eq!(listed[0].client_id.as_deref(), Some("kobo-keep"));
}

#[tokio::test]
async fn patch_for_an_unknown_book_answers_204_without_ingesting() {
    let (app, pool, user, _device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    let res = app
        .oneshot(request(
            "PATCH",
            &annotations_uri("not-in-this-library"),
            Some(HW_ID),
            Some(&upload_body("kobo-ann-1", "yellow", None)),
        ))
        .await
        .unwrap();
    // Device noise (sideloaded book) must not error-loop the sync.
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert!(db::annotations::list_highlights(&pool, user, &uuid)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn patch_stores_a_derived_range_cfi_when_kepub_and_source_are_on_disk() {
    use db::test_support::{build_test_epub, build_test_kepub, make_test_dir, EnvVarGuard};

    let (app, pool, user, _device) = fixture().await;

    // A real on-disk library with one chapter, plus its kepubify-shaped twin
    // in the kepub cache dir — the two walks annotation_cfis aligns.
    let source_xhtml = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head>
<body><p>First sentence here. Second sentence follows.</p></body></html>"#;
    let kepub_xhtml = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head>
<body><p><span class="koboSpan" id="kobo.1.1">First sentence here. </span><span class="koboSpan" id="kobo.1.2">Second sentence follows.</span></p></body></html>"#;
    let dir = make_test_dir("rs_patch_cfi");
    std::fs::write(
        dir.join("book.epub"),
        build_test_epub(&[("c1.xhtml", source_xhtml)]),
    )
    .unwrap();
    db::replace_books(
        &pool,
        dir.to_str().unwrap(),
        vec![db::ebook::IndexedBook {
            metadata: omnibus_shared::EbookMetadata {
                filename: "book.epub".into(),
                title: Some("Book".into()),
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
            word_count: None,
        }],
    )
    .await
    .unwrap();
    let books = db::list_books(&pool, dir.to_str().unwrap()).await.unwrap();
    let book = books.first().unwrap();
    let uuid = book.unique_identifier.clone().unwrap();
    let kepub_dir = dir.join("kepub");
    std::fs::create_dir_all(&kepub_dir).unwrap();
    std::fs::write(
        kepub_dir.join(format!("{}.kepub.epub", book.id)),
        build_test_kepub(&[("c1.xhtml", kepub_xhtml)]),
    )
    .unwrap();
    let _guard = EnvVarGuard::set("OMNIBUS_KEPUB_DIR", Some(kepub_dir.to_str().unwrap()));

    let body = json!({ "updatedAnnotations": [{
        "id": "kobo-derived-1",
        "type": "highlight",
        "highlightColor": "yellow",
        "highlightedText": "Second sentence follows.",
        "location": { "span": {
            "chapterFilename": "c1.xhtml",
            "startPath": "span#kobo\\.1\\.2",
            "startChar": 0,
            "endPath": "span#kobo\\.1\\.2",
            "endChar": 24
        }}
    }]});
    let res = app
        .oneshot(request(
            "PATCH",
            &annotations_uri(&uuid),
            Some(HW_ID),
            Some(&body),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    let listed = db::annotations::list_highlights(&pool, user, &uuid)
        .await
        .unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(
        listed[0].epub_cfi_range.as_deref(),
        Some("epubcfi(/6/2!/4/2,/1:21,/1:45)"),
        "web/iOS readers need the derived range CFI to paint the highlight"
    );
}

#[tokio::test]
async fn checkforchanges_reports_an_ingested_book_and_goes_quiet_after_the_get_drains() {
    let (app, pool, _user, _device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    // Nothing anywhere: quiet.
    assert!(check_for_changes(&app).await.is_empty());

    // Upload → the book is reported until the device fetches it.
    let res = app
        .clone()
        .oneshot(request(
            "PATCH",
            &annotations_uri(&uuid),
            Some(HW_ID),
            Some(&upload_body("kobo-ann-1", "yellow", None)),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    assert_eq!(check_for_changes(&app).await, vec![uuid.clone()]);

    // Drain the GET → acked → quiet.
    let res = app
        .clone()
        .oneshot(request("GET", &annotations_uri(&uuid), Some(HW_ID), None))
        .await
        .unwrap();
    let _ = body_json(res).await;
    assert!(check_for_changes(&app).await.is_empty());
}

#[tokio::test]
async fn checkforchanges_never_reports_an_unadopted_pair() {
    let (app, pool, _user, _device) = fixture().await;
    let _uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    // AC5's other half: a book with no annotations and no PATCH history must
    // not be offered for a GET at all.
    assert!(check_for_changes(&app).await.is_empty());
}

#[tokio::test]
async fn patch_with_skipped_entries_does_not_adopt_the_pair() {
    let (app, pool, _user, device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

    // One good entry, one idless entry we drop. Ingest keeps the good one,
    // but adoption must wait for a fully-clean upload — the dropped entry
    // could be backlog the next GET's omission would wipe.
    let body = json!({
        "updatedAnnotations": [
            upload_body("kobo-ann-1", "yellow", None)["updatedAnnotations"][0],
            { "highlightColor": "green", "location": {} }
        ]
    });
    let res = app
        .clone()
        .oneshot(request(
            "PATCH",
            &annotations_uri(&uuid),
            Some(HW_ID),
            Some(&body),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);

    assert!(!db::kobo::annotations::is_adopted(&pool, device, &uuid)
        .await
        .unwrap());
    // The GET still answers 200 — the served set is non-empty, which is safe
    // by itself; adoption only gates the empty answer.
    let res = app
        .oneshot(request("GET", &annotations_uri(&uuid), Some(HW_ID), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
}

#[tokio::test]
async fn web_side_delete_flows_down_as_a_change_report_and_omission() {
    let (app, pool, user, _device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    upload_and_ack(&app, &uuid, "kobo-ann-1", "yellow", None).await;
    upload_and_ack(&app, &uuid, "kobo-ann-2", "blue", None).await;
    assert!(check_for_changes(&app).await.is_empty());

    // AC4: delete one in the web UI → checkforchanges resurfaces the book →
    // the GET body omits the deleted id — omission IS the tombstone.
    let id = db::annotations::highlight_id_for_client_id(&pool, user, "kobo-ann-1")
        .await
        .unwrap()
        .unwrap();
    db::annotations::delete_highlight(&pool, user, id)
        .await
        .unwrap();

    assert_eq!(check_for_changes(&app).await, vec![uuid.clone()]);
    let res = app
        .clone()
        .oneshot(request("GET", &annotations_uri(&uuid), Some(HW_ID), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let ids: Vec<&str> = body["annotations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(ids, vec!["kobo-ann-2"]);
    assert!(check_for_changes(&app).await.is_empty(), "drain re-acks");
}

#[tokio::test]
async fn web_side_recolor_and_note_flow_down_on_the_next_get() {
    let (app, pool, user, _device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    upload_and_ack(&app, &uuid, "kobo-ann-1", "yellow", None).await;

    let id = db::annotations::highlight_id_for_client_id(&pool, user, "kobo-ann-1")
        .await
        .unwrap()
        .unwrap();
    db::annotations::update_highlight_color(
        &pool,
        user,
        id,
        omnibus_shared::HighlightColor::Violet,
    )
    .await
    .unwrap();
    db::annotations::update_highlight_note(&pool, user, id, Some("from the web"))
        .await
        .unwrap();

    assert_eq!(check_for_changes(&app).await, vec![uuid.clone()]);
    let res = app
        .oneshot(request("GET", &annotations_uri(&uuid), Some(HW_ID), None))
        .await
        .unwrap();
    let body = body_json(res).await;
    let a = &body["annotations"][0];
    assert_eq!(a["highlightColor"], "purple");
    assert_eq!(a["noteText"], "from the web");
    assert_eq!(a["type"], "note", "a note-bearing row serves as a note");
}

#[tokio::test]
async fn a_device_of_another_user_never_sees_the_annotations() {
    let (app, pool, _user, _device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    upload_and_ack(&app, &uuid, "kobo-ann-1", "yellow", None).await;

    let other = auth_test_support::create_user(&pool, "other-reader").await;
    let other_device = db::kobo_devices::create_device(&pool, other.id, "Other Kobo")
        .await
        .unwrap();
    db::kobo_devices::learn_kobo_device_id(&pool, other_device.id, "hw-other")
        .await
        .unwrap();

    // Their checkforchanges is quiet, and their GET on the same book 304s —
    // there is nothing of *theirs* to serve.
    let res = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v3/content/checkforchanges",
            Some("hw-other"),
            Some(&json!([])),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(res).await, json!([]));
    let res = app
        .oneshot(request(
            "GET",
            &annotations_uri(&uuid),
            Some("hw-other"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_MODIFIED);
}

#[tokio::test]
async fn a_second_device_of_the_same_user_is_offered_the_existing_set() {
    let (app, pool, user, _device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    upload_and_ack(&app, &uuid, "kobo-ann-1", "yellow", None).await;

    let second = db::kobo_devices::create_device(&pool, user, "Second Kobo")
        .await
        .unwrap();
    db::kobo_devices::learn_kobo_device_id(&pool, second.id, "hw-second")
        .await
        .unwrap();

    // The set is non-empty, so serving it to a fresh device is safe — and
    // checkforchanges offers it without any PATCH from that device.
    let res = app
        .clone()
        .oneshot(request(
            "POST",
            "/api/v3/content/checkforchanges",
            Some("hw-second"),
            Some(&json!([])),
        ))
        .await
        .unwrap();
    assert_eq!(body_json(res).await, json!([uuid.clone()]));
    let res = app
        .oneshot(request(
            "GET",
            &annotations_uri(&uuid),
            Some("hw-second"),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["annotations"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn content_id_with_a_kepub_chapter_suffix_resolves_to_the_book() {
    let (app, pool, _user, _device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    upload_and_ack(&app, &uuid, "kobo-ann-1", "yellow", None).await;

    // The device sends the chapter-scoped id URL-encoded — it is one path
    // segment; axum decodes it after matching.
    let scoped = format!("{uuid}!!OEBPS%2Fch1.xhtml");
    let res = app
        .oneshot(request("GET", &annotations_uri(&scoped), Some(HW_ID), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["annotations"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn user_storage_metadata_returns_the_empty_stub() {
    let (app, _pool, _user, _device) = fixture().await;
    let res = app
        .oneshot(request(
            "GET",
            "/api/UserStorage/Metadata",
            Some(HW_ID),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        body_json(res).await,
        json!({ "continuationToken": null, "metadata": [] })
    );
}

#[tokio::test]
async fn get_annotations_returns_500_when_pool_is_closed() {
    let (app, pool, _user, _device) = fixture().await;
    pool.close().await;

    let res = app
        .oneshot(request(
            "GET",
            &annotations_uri("any-book-uuid"),
            Some(HW_ID),
            None,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn patch_annotations_returns_500_when_pool_is_closed() {
    let (app, pool, _user, _device) = fixture().await;
    pool.close().await;

    let res = app
        .oneshot(request(
            "PATCH",
            &annotations_uri("any-book-uuid"),
            Some(HW_ID),
            Some(&upload_body("kobo-ann-1", "yellow", None)),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

#[tokio::test]
async fn check_for_changes_returns_500_when_pool_is_closed() {
    let (app, pool, _user, _device) = fixture().await;
    pool.close().await;

    let res = app
        .oneshot(request(
            "POST",
            "/api/v3/content/checkforchanges",
            Some(HW_ID),
            Some(&json!([])),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

mod dto_tests {
    use super::super::dto::*;
    use omnibus_shared::HighlightColor;
    use serde_json::json;

    #[test]
    fn parse_patch_accepts_variant_delete_shapes_and_deleted_flags() {
        let body = json!({
            "updatedAnnotations": [
                { "id": "keep", "highlightColor": "blue", "location": {} },
                { "id": "flagged", "isDeleted": true, "location": {} }
            ],
            "deletedAnnotationIds": ["bare-id"],
            "deletedAnnotations": [{ "id": "object-id" }],
            "removedAnnotations": ["removed-id"]
        });
        let parsed = parse_patch(&body);
        assert_eq!(parsed.skipped, 0);
        assert_eq!(parsed.updates.len(), 1);
        assert_eq!(parsed.updates[0].client_id, "keep");
        let mut deleted = parsed.deleted_ids.clone();
        deleted.sort();
        assert_eq!(
            deleted,
            vec!["bare-id", "flagged", "object-id", "removed-id"]
        );
    }

    #[test]
    fn parse_patch_skips_capped_and_idless_entries_without_dropping_the_rest() {
        let body = json!({
            "updatedAnnotations": [
                { "id": "ok", "location": {} },
                { "location": {} },
                { "id": "x".repeat(65), "location": {} },
                { "id": "long-note", "noteText": "n".repeat(4097), "location": {} },
                "not-an-object"
            ]
        });
        let parsed = parse_patch(&body);
        assert_eq!(parsed.updates.len(), 1);
        assert_eq!(parsed.updates[0].client_id, "ok");
        assert_eq!(parsed.skipped, 4);
    }

    #[test]
    fn parse_patch_treats_a_non_object_body_as_one_skip() {
        let parsed = parse_patch(&json!([1, 2, 3]));
        assert!(parsed.updates.is_empty());
        assert!(parsed.deleted_ids.is_empty());
        assert_eq!(parsed.skipped, 1);
    }

    #[test]
    fn parse_patch_lets_a_later_duplicate_win_and_a_delete_beat_an_update() {
        let body = json!({
            "updatedAnnotations": [
                { "id": "dup", "highlightColor": "blue", "location": {} },
                { "id": "dup", "highlightColor": "purple", "location": {} },
                { "id": "doomed", "highlightColor": "green", "location": {} }
            ],
            "deletedAnnotationIds": ["doomed"]
        });
        let parsed = parse_patch(&body);
        assert_eq!(parsed.updates.len(), 1);
        assert_eq!(parsed.updates[0].client_id, "dup");
        assert_eq!(parsed.updates[0].color, HighlightColor::Violet);
        assert_eq!(parsed.deleted_ids, vec!["doomed"]);
    }

    #[test]
    fn color_from_kobo_defaults_unknown_hex_and_missing_to_amber() {
        assert_eq!(color_from_kobo(Some("#FFDD00")), HighlightColor::Amber);
        assert_eq!(color_from_kobo(Some("chartreuse")), HighlightColor::Amber);
        assert_eq!(color_from_kobo(None), HighlightColor::Amber);
        assert_eq!(color_from_kobo(Some("PINK")), HighlightColor::Rose);
    }

    #[test]
    fn color_round_trip_is_stable_for_every_palette_color() {
        for color in [
            HighlightColor::Amber,
            HighlightColor::Green,
            HighlightColor::Blue,
            HighlightColor::Rose,
            HighlightColor::Violet,
        ] {
            assert_eq!(color_from_kobo(Some(color_to_kobo(color))), color);
        }
    }

    #[test]
    fn parse_content_id_strips_the_chapter_suffix() {
        assert_eq!(parse_content_id("uuid-1!!OEBPS/ch1.xhtml"), "uuid-1");
        assert_eq!(parse_content_id("uuid-1"), "uuid-1");
        assert_eq!(parse_content_id(""), "");
    }
}
