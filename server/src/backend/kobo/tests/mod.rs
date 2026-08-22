//! HTTP-layer contract tests for the wireless Kobo routes, driving
//! `kobo_router` via `oneshot` against an in-memory DB. The device sequence
//! (sync → metadata → state) is replayed at the HTTP layer because Playwright
//! can't drive a Kobo. The shared fixtures and that end-to-end sequence live
//! here; the per-endpoint suites are split into the sibling modules below.

mod analytics;
mod auth;
mod content;
mod resources;
mod state;
mod state_statistics;
mod sync;

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
    response::Response,
    Router,
};
use omnibus_db as db;
use omnibus_shared::ReadStatus;
use serde_json::Value;
use sqlx::SqlitePool;
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;

use super::*;

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

/// Seed one real book on disk — a copy of the committed EPUB fixture under a
/// fresh scan root — so [`download`](super::resources::download) has real
/// bytes to serve. Mirrors the on-disk setup
/// `download_bakes_a_metadata_override_into_the_plain_epub_fallback` uses,
/// minus the override (that sibling test cleans up its tempdir via
/// `remove_dir_all`; this one deliberately doesn't, since each run gets a
/// fresh pid-scoped path and CI wipes `/tmp` between runs).
async fn seed_downloadable_book(pool: &SqlitePool, uuid: &str, title: &str, author: &str) {
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let lib_dir = std::env::temp_dir().join(format!("omnibus_kobo_contract_{pid}_{nanos}_{uuid}"));
    std::fs::create_dir_all(&lib_dir).unwrap();
    let fixture_epub = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../test_data/epubs/generated/alpha.epub");
    std::fs::copy(&fixture_epub, lib_dir.join("alpha.epub")).unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(lib_dir.to_str().unwrap())
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, last_modified) \
         VALUES (?, ?, ?, ?, 1700000000)",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(lib_dir.to_str().unwrap())
    .bind(title)
    .execute(pool)
    .await
    .unwrap();
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', 'alpha', 0)",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap();
    let author_id: i64 = sqlx::query_scalar("INSERT INTO authors (name) VALUES (?) RETURNING id")
        .bind(author)
        .fetch_one(pool)
        .await
        .unwrap();
    sqlx::query("INSERT INTO books_authors_link (book, author, position) VALUES (?, ?, 0)")
        .bind(book_id)
        .bind(author_id)
        .execute(pool)
        .await
        .unwrap();
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

/// #928 contract test: replays the full wireless device sequence at the HTTP
/// layer — `initialization -> auth/device -> library/sync -> download -> PUT
/// state -> GET state` — against `kobo_router` over `sqlite::memory:`.
///
/// **This is a synthetic golden fixture, not a real-device capture.** No
/// physical Kobo is available in this sandboxed environment; every payload
/// shape below (envelope field names, the `Resources` override keys, the
/// `NewEntitlement`/`StateResponse` field names, the `KoboSpan` location
/// format) is instead pinned against the DTOs this router already
/// implements (`kobo::dto`, `kobo::store_resources`, `kobo::state`) — the
/// same reference shapes `docs/kobo.md` documents as sourced from
/// Calibre-Web / bookorbit rather than a packet capture. See
/// `docs/kobo-smoke-test.md` for the manual real-hardware checklist that
/// gates advertising a device's wireless sync URL to a user.
///
/// AC4: every step asserts response **body shape**, not just status code —
/// a regression that changed a field name or dropped a value (rather than
/// the status code) fails this test. The final `GET state` closes the loop:
/// it proves the `PUT` actually persisted the device's status and bookmark,
/// not merely that the endpoint answered `200`.
#[tokio::test]
async fn full_device_sequence_replays_initialization_through_state_put() {
    let _kepubify_absent =
        db::test_support::EnvVarGuard::set("OMNIBUS_KEPUBIFY_PATH", Some("/no/such/kepubify"));
    let (app, pool, token, uid) = fixture().await;
    let uuid = "62e1c9f0-0000-4000-8000-000000000928";
    seed_downloadable_book(&pool, uuid, "The Golden Fixture", "Ada Lovelace").await;
    opt_in(&pool, uid, &[uuid.to_owned()]).await;

    // --- 1. GET v1/initialization -----------------------------------------
    let res = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/initialization")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get("x-kobo-apitoken").unwrap(),
        "e30=",
        "missing/wrong x-kobo-apitoken makes the device reject the whole map"
    );
    let init = body_json(res).await;
    assert_eq!(
        init["Resources"]["library_sync"],
        format!("http://omni.test/kobo/{token}/v1/library/sync")
    );

    // --- 2. POST v1/auth/device ---------------------------------------------
    let res = app
        .clone()
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
    let auth_envelope = body_json(res).await;
    assert_eq!(auth_envelope["TokenType"], "Bearer");
    for field in ["AccessToken", "RefreshToken", "TrackingId", "UserKey"] {
        assert!(
            auth_envelope[field].as_str().is_some_and(|s| !s.is_empty()),
            "{field} should be present and non-empty"
        );
    }

    // --- 3. GET v1/library/sync ---------------------------------------------
    let res = app
        .clone()
        .oneshot(get(format!("/kobo/{token}/v1/library/sync")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert!(
        res.headers().get("x-kobo-sync").is_none(),
        "a single-book first sync must not page"
    );
    let sync = body_json(res).await;
    let items = sync.as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "first sync emits exactly one NewEntitlement"
    );
    let ent = &items[0]["NewEntitlement"];
    assert_eq!(ent["BookEntitlement"]["Id"], uuid);
    assert_eq!(ent["BookEntitlement"]["IsRemoved"], false);
    assert_eq!(ent["BookMetadata"]["Title"], "The Golden Fixture");
    assert_eq!(
        ent["BookMetadata"]["ContributorRoles"][0]["Name"],
        "Ada Lovelace"
    );
    let download_url = ent["BookMetadata"]["DownloadUrls"][0]["Url"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(
        ent["BookMetadata"]["DownloadUrls"][0]["Format"], "KEPUB",
        "an EPUB-bearing book advertises the KEPUB conversion"
    );
    assert_eq!(
        download_url,
        format!("http://omni.test/kobo/{token}/v1/download/{uuid}")
    );
    assert_eq!(
        ent["ReadingState"]["StatusInfo"]["Status"], "ReadyToRead",
        "an untouched book starts ReadyToRead"
    );

    // --- 4. GET v1/download/<uuid> ------------------------------------------
    let res = app
        .clone()
        .oneshot(get(download_url.replace("http://omni.test", "")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers().get(axum::http::header::CONTENT_TYPE).unwrap(),
        "application/epub+zip"
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    epub::doc::EpubDoc::from_reader(std::io::Cursor::new(bytes.to_vec()))
        .expect("downloaded bytes are a valid EPUB");
    let device_id = db::kobo_devices::resolve_device_by_token(&pool, &token)
        .await
        .unwrap()
        .unwrap()
        .device_id;
    let downloaded_at: Option<i64> = sqlx::query_scalar(
        "SELECT downloaded_at FROM kobo_annotations_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device_id)
    .bind(uuid)
    .fetch_optional(&pool)
    .await
    .unwrap()
    .flatten();
    assert!(
        downloaded_at.is_some(),
        "a successful download must record the device as holding the book"
    );

    // --- 5. PUT v1/library/<uuid>/state --------------------------------------
    let location = serde_json::json!({
        "Source": "text/part0001.html",
        "Type": "KoboSpan",
        "Value": "kobo.3.2",
    });
    let put_body = serde_json::json!({
        "ReadingStates": [{
            "StatusInfo": { "Status": "Reading" },
            "CurrentBookmark": {
                "ProgressPercent": 42,
                "Location": location,
                "LastModified": "2026-01-02T03:04:05Z",
            },
        }],
    });
    let res = app
        .clone()
        .oneshot(state_put(&token, uuid, put_body))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let put_response = body_json(res).await;
    assert_eq!(put_response["RequestResult"], "Success");
    let update_results = put_response["UpdateResults"].as_array().unwrap();
    assert_eq!(update_results.len(), 1);
    assert_eq!(update_results[0]["EntitlementId"], uuid);
    assert_eq!(update_results[0]["StatusInfoResult"]["Result"], "Success");
    assert_eq!(
        update_results[0]["CurrentBookmarkResult"]["Result"],
        "Success"
    );

    // --- 6. GET v1/library/<uuid>/state — proves the PUT actually persisted -
    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/library/{uuid}/state")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let states = body_json(res).await;
    let states = states.as_array().unwrap();
    assert_eq!(states.len(), 1);
    let state = &states[0];
    assert_eq!(state["EntitlementId"], uuid);
    assert_eq!(
        state["StatusInfo"]["Status"], "Reading",
        "the status set by the PUT must be echoed back, not the stale default"
    );
    assert_eq!(state["CurrentBookmark"]["ProgressPercent"], 42);
    assert_eq!(
        state["CurrentBookmark"]["Location"], location,
        "the KoboSpan location is echoed back verbatim"
    );
}
