//! The byte- and metadata-serving routes: `/v1/library/{uuid}/metadata`,
//! the download (kepub, plain-EPUB fallback, CBZ passthrough), and the cover
//! image with its conditional-request handling.

use axum::{
    body::{to_bytes, Body},
    http::{Request, StatusCode},
};
use omnibus_db::{self as db, test_support::seed_synced_ebook};
use tower::ServiceExt;

use super::{body_json, fixture, get, seed_book_with_kepub_cache};

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
async fn download_returns_404_for_unknown_uuid() {
    let (app, _pool, token, _uid) = fixture().await;
    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/download/does-not-exist")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// #1647: the book row (and its `book_files` entry) resolves fine — only the
/// actual bytes are missing, so `serve_download` 404s from `conditional::open`
/// failing rather than from the earlier uuid/id lookups. That later 404 must
/// close the same gap the id-lookup 404 above closes: a device that never
/// got a file must not be recorded as holding it, or the next
/// `checkforchanges` cycle silently acks annotations away from a device that
/// has nothing to show them in.
#[tokio::test]
async fn download_does_not_record_download_state_when_the_file_open_fails() {
    // Force kepubify absent so this deterministically takes the plain-EPUB
    // fallback and reaches `serve_download` with a `book_file_path` pointing
    // at bytes that were never written to disk.
    let _kepubify_absent =
        db::test_support::EnvVarGuard::set("OMNIBUS_KEPUBIFY_PATH", Some("/no/such/kepubify"));
    let (app, pool, token, _uid) = fixture().await;
    let device_id = db::kobo_devices::resolve_device_by_token(&pool, &token)
        .await
        .unwrap()
        .unwrap()
        .device_id;
    let uuid = seed_synced_ebook(&pool, "phantom.epub", "Phantom", "Nobody").await;

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/download/{uuid}")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    let recorded: Option<i64> = sqlx::query_scalar(
        "SELECT downloaded_at FROM kobo_annotations_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device_id)
    .bind(&uuid)
    .fetch_optional(&pool)
    .await
    .unwrap()
    .flatten();
    assert!(
        recorded.is_none(),
        "a 404'd download must not mark the device as holding the book"
    );
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

/// #1741: a CBZ-only book downloads as the raw archive (no conversion
/// attempt, so no kepubify guard) and still runs the #1647 bookkeeping.
#[tokio::test]
async fn download_serves_the_cbz_archive_as_is_for_a_cbz_only_book() {
    let (app, pool, token, _uid) = fixture().await;
    let device_id = db::kobo_devices::resolve_device_by_token(&pool, &token)
        .await
        .unwrap()
        .unwrap()
        .device_id;

    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("omnibus_kobo_dl_cbz_{pid}_{nanos}"));
    std::fs::create_dir_all(&tmp).unwrap();
    let archive = db::test_support::build_stored_zip(&[("p1.jpg", b"page-one")]);
    std::fs::write(tmp.join("aurora.cbz"), &archive).unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(tmp.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "66666666-6666-6666-6666-666666666666";
    sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, last_modified) \
         VALUES (?, ?, ?, 'Aurora', 1)",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(tmp.to_str().unwrap())
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES ((SELECT id FROM books WHERE uuid = ?), 'CBZ', 'aurora', 0)",
    )
    .bind(uuid)
    .execute(&pool)
    .await
    .unwrap();

    let res = app
        .oneshot(get(format!("/kobo/{token}/v1/download/{uuid}")))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/vnd.comicbook+zip"),
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], &archive[..], "the archive streams as-is");

    let recorded: Option<i64> = sqlx::query_scalar(
        "SELECT downloaded_at FROM kobo_annotations_sync WHERE device_id = ? AND book_uuid = ?",
    )
    .bind(device_id)
    .bind(uuid)
    .fetch_optional(&pool)
    .await
    .unwrap()
    .flatten();
    assert!(
        recorded.is_some(),
        "a served CBZ must record the device as holding the book (#1647 gate)"
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
