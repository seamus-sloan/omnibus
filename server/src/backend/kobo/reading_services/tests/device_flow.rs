//! The device's own writes and reads: GET's device-id, unknown-book,
//! oversized-content-id and not-modified answers, PATCH's validation,
//! replay safety, tolerance of garbage and variant delete shapes, the
//! unknown-book 204, the derived range CFI, and the pool-closed 500s.

use axum::http::StatusCode;
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use serde_json::json;
use tower::ServiceExt;

use super::{annotations_uri, body_json, fixture, request, upload_and_ack, upload_body, HW_ID};

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
    assert_eq!(a["highlightColor"], "#F6F3B3");
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
    let (app, pool, user, device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    upload_and_ack(&app, &pool, device, &uuid, "kobo-keep", "green", None).await;
    upload_and_ack(&app, &pool, device, &uuid, "kobo-drop", "yellow", None).await;

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
