//! Web-origin annotations flowing down to the device: the mixed served
//! set, web edits, recolors and deletes reported by checkforchanges and
//! drained by the next GET, the AC5 first-sync guard for a book the device
//! has not downloaded, unadopted pairs, other users' and second devices,
//! the KEPUB chapter suffix, and the user-storage stub.

use axum::{http::StatusCode, Router};
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use serde_json::{json, Value};
use sqlx::SqlitePool;
use tower::ServiceExt;

use super::{annotations_uri, body_json, fixture, request, upload_and_ack, upload_body, HW_ID};
use crate::auth::test_support as auth_test_support;

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

/// On-disk one-chapter book plus its kepubify-shaped twin in the kepub
/// cache dir — the pair the CFI↔KoboSpan converters walk. Returns the
/// book's uuid, the fixture dir, and the env guard pinning
/// `OMNIBUS_KEPUB_DIR`; hold both to keep the fixture alive.
async fn disk_book(
    pool: &SqlitePool,
    tag: &str,
) -> (String, std::path::PathBuf, db::test_support::EnvVarGuard) {
    use db::test_support::{build_test_epub, build_test_kepub, make_test_dir, EnvVarGuard};

    let source_xhtml = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head>
<body><p>First sentence here. Second sentence follows.</p></body></html>"#;
    let kepub_xhtml = r#"<?xml version="1.0"?><html xmlns="http://www.w3.org/1999/xhtml"><head><title>t</title></head>
<body><p><span class="koboSpan" id="kobo.1.1">First sentence here. </span><span class="koboSpan" id="kobo.1.2">Second sentence follows.</span></p></body></html>"#;
    let dir = make_test_dir(&format!("rs_downsync_{tag}"));
    std::fs::write(
        dir.join("book.epub"),
        build_test_epub(&[("c1.xhtml", source_xhtml)]),
    )
    .unwrap();
    db::replace_books(
        pool,
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
    let books = db::list_books(pool, dir.to_str().unwrap()).await.unwrap();
    let book = books.first().unwrap();
    let uuid = book.unique_identifier.clone().unwrap();
    let kepub_dir = dir.join("kepub");
    std::fs::create_dir_all(&kepub_dir).unwrap();
    std::fs::write(
        kepub_dir.join(format!("{}.kepub.epub", book.id)),
        build_test_kepub(&[("c1.xhtml", kepub_xhtml)]),
    )
    .unwrap();
    let guard = EnvVarGuard::set("OMNIBUS_KEPUB_DIR", Some(kepub_dir.to_str().unwrap()));
    (uuid, dir, guard)
}

/// The web-reader highlight covering "Second sentence follows." in the
/// [`disk_book`] fixture — derives to all of `kobo.1.2`.
const WEB_CFI: &str = "epubcfi(/6/2!/4/2,/1:21,/1:45)";

/// Create a web-origin highlight (CFI anchor only) directly in the store,
/// as the web reader's create RPC would.
async fn create_web_highlight(pool: &SqlitePool, user: i64, uuid: &str, cfi: &str) -> i64 {
    db::annotations::create_highlight(
        pool,
        user,
        &omnibus_shared::CreateHighlight {
            book_uuid: uuid.to_string(),
            epub_cfi_range: cfi.to_string(),
            color: omnibus_shared::HighlightColor::Green,
            text: Some("Second sentence follows.".into()),
            client_id: Some("web-1".into()),
        },
    )
    .await
    .unwrap()
    .id
}

/// The annotation entries of a GET body, as `(id, entry)` pairs — lookup
/// by id, since two rows created in the same second have no stable order.
fn annotations_by_id(body: &Value) -> std::collections::BTreeMap<String, Value> {
    body["annotations"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| (a["id"].as_str().unwrap().to_string(), a.clone()))
        .collect()
}

#[tokio::test]
async fn get_serves_a_mixed_set_of_device_and_converted_web_annotations() {
    let (app, pool, user, device) = fixture().await;
    let (uuid, _dir, _guard) = disk_book(&pool, "mixed").await;

    // The real device flow: PATCH the backlog (adopting the pair), a web
    // highlight lands, checkforchanges reports the never-acked book, and
    // the GET materializes the web row and serves both origins in one set.
    let res = app
        .clone()
        .oneshot(request(
            "PATCH",
            &annotations_uri(&uuid),
            Some(HW_ID),
            Some(&upload_body("kobo-native-1", "yellow", None)),
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NO_CONTENT);
    db::kobo::annotations::mark_downloaded(&pool, device, &uuid)
        .await
        .unwrap();
    create_web_highlight(&pool, user, &uuid, WEB_CFI).await;

    assert_eq!(check_for_changes(&app).await, vec![uuid.clone()]);
    let res = app
        .clone()
        .oneshot(request("GET", &annotations_uri(&uuid), Some(HW_ID), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    let by_id = annotations_by_id(&body);
    assert_eq!(by_id.len(), 2);
    assert!(by_id.contains_key("kobo-native-1"));
    // The converted row carries a real KoboSpan range, not a null or an
    // echo of the CFI.
    let web = &by_id["web-1"];
    assert_eq!(web["location"]["span"]["chapterFilename"], "c1.xhtml");
    assert_eq!(web["location"]["span"]["startPath"], "span#kobo\\.1\\.2");
    assert_eq!(web["location"]["span"]["endChar"], 24);
    assert_eq!(web["highlightColor"], "#C6E09E");

    // The drained GET acked the mixed fingerprint: the book goes quiet.
    assert!(check_for_changes(&app).await.is_empty());
}

#[tokio::test]
async fn web_edit_of_a_converted_row_reports_and_flows_down_on_the_next_get() {
    let (app, pool, user, device) = fixture().await;
    let (uuid, _dir, _guard) = disk_book(&pool, "edit").await;
    upload_and_ack(&app, &pool, device, &uuid, "kobo-native-1", "yellow", None).await;
    let web_id = create_web_highlight(&pool, user, &uuid, WEB_CFI).await;
    // Drain the mixed set so the device is fully caught up.
    let res = app
        .clone()
        .oneshot(request("GET", &annotations_uri(&uuid), Some(HW_ID), None))
        .await
        .unwrap();
    let _ = body_json(res).await;
    assert!(check_for_changes(&app).await.is_empty());

    // A web-side recolor of the converted row moves the fingerprint and
    // re-delivers with the new color (AC3).
    db::annotations::update_highlight_color(
        &pool,
        user,
        web_id,
        omnibus_shared::HighlightColor::Rose,
    )
    .await
    .unwrap();
    assert_eq!(check_for_changes(&app).await, vec![uuid.clone()]);
    let res = app
        .clone()
        .oneshot(request("GET", &annotations_uri(&uuid), Some(HW_ID), None))
        .await
        .unwrap();
    let body = body_json(res).await;
    let by_id = annotations_by_id(&body);
    assert_eq!(by_id["web-1"]["highlightColor"], "#E8AFCF");
}

#[tokio::test]
async fn an_underivable_web_cfi_degrades_to_not_served_never_an_error() {
    let (app, pool, user, device) = fixture().await;
    let (uuid, _dir, _guard) = disk_book(&pool, "degrade").await;

    upload_and_ack(&app, &pool, device, &uuid, "kobo-native-1", "yellow", None).await;
    // A point CFI can never derive a range: the row must simply stay off
    // the wire — 200 with the device's own set, no 500, no null location.
    create_web_highlight(&pool, user, &uuid, "epubcfi(/6/2!/4/2/1:0)").await;

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
    assert_eq!(ids, vec!["kobo-native-1"]);
    // The failed conversion leaves the fingerprint unmoved: no re-report loop.
    assert!(check_for_changes(&app).await.is_empty());
}

#[tokio::test]
async fn checkforchanges_reports_an_ingested_book_and_goes_quiet_after_the_get_drains() {
    let (app, pool, _user, device) = fixture().await;
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
    db::kobo::annotations::mark_downloaded(&pool, device, &uuid)
        .await
        .unwrap();

    // Drain the GET → acked → quiet.
    let res = app
        .clone()
        .oneshot(request("GET", &annotations_uri(&uuid), Some(HW_ID), None))
        .await
        .unwrap();
    let _ = body_json(res).await;
    assert!(check_for_changes(&app).await.is_empty());
}

/// #1647 (AC1/AC3): a GET for a book this device never downloaded still
/// serves the set — a factory-reset or second device must be able to fetch
/// existing annotations — but draining it must not stick as an ack. Without
/// the download-state gate, `checkforchanges` would go quiet here even
/// though the device discarded the annotations for lack of the book file.
#[tokio::test]
async fn get_serves_but_does_not_ack_a_book_the_device_has_not_downloaded() {
    let (app, pool, _user, device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;

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

    // No `mark_downloaded` here — the device is adopted (it PATCHed) but has
    // never fetched the book file over `download`.
    let res = app
        .clone()
        .oneshot(request("GET", &annotations_uri(&uuid), Some(HW_ID), None))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body = body_json(res).await;
    assert_eq!(body["annotations"].as_array().unwrap().len(), 1);

    // The GET served bytes, but the ack never stuck: still reportable.
    assert_eq!(check_for_changes(&app).await, vec![uuid.clone()]);
    let acked: Option<String> = sqlx::query_scalar(
        "SELECT acked_fingerprint FROM kobo_annotations_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device)
    .bind(&uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert!(acked.is_none());

    // Only once the device actually downloads the book does the ack stick.
    db::kobo::annotations::mark_downloaded(&pool, device, &uuid)
        .await
        .unwrap();
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
    let (app, pool, user, device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    upload_and_ack(&app, &pool, device, &uuid, "kobo-ann-1", "yellow", None).await;
    upload_and_ack(&app, &pool, device, &uuid, "kobo-ann-2", "blue", None).await;
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
    let (app, pool, user, device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    upload_and_ack(&app, &pool, device, &uuid, "kobo-ann-1", "yellow", None).await;

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
    // Violet has no fifth Kobo swatch, so it renders as rose's hex (see color_to_kobo's doc comment).
    assert_eq!(a["highlightColor"], "#E8AFCF");
    assert_eq!(a["noteText"], "from the web");
    assert_eq!(a["type"], "note", "a note-bearing row serves as a note");
}

#[tokio::test]
async fn a_device_of_another_user_never_sees_the_annotations() {
    let (app, pool, _user, device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    upload_and_ack(&app, &pool, device, &uuid, "kobo-ann-1", "yellow", None).await;

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
    let (app, pool, user, device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    upload_and_ack(&app, &pool, device, &uuid, "kobo-ann-1", "yellow", None).await;

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
    let (app, pool, _user, device) = fixture().await;
    let uuid = seed_synced_ebook(&pool, "dune.epub", "Dune", "Herbert").await;
    upload_and_ack(&app, &pool, device, &uuid, "kobo-ann-1", "yellow", None).await;

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
