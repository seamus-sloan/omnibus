//! `GET /api/ebooks/{uuid}/kepub` and `/download`: the KEPUB cache hit and
//! its plain-EPUB fallback, the attachment disposition, and the metadata
//! overrides baked into the exported EPUB.

use axum::{body::to_bytes, http::StatusCode};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

use super::super::*;
use super::{content_disposition, seed_epub_on_disk};

#[tokio::test]
async fn api_get_ebook_kepub_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/ebooks/some-uuid/kepub"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// AC1 (#1810): `can_download = 0` must 403 the KEPUB download route, the
/// same guard the OPDS acquisition delegates already enforce.
#[tokio::test]
async fn api_get_ebook_kepub_returns_403_when_user_cannot_download() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    revoke_can_download(&pool, user.id).await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/ebooks/some-uuid/kepub", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_get_ebook_kepub_returns_404_for_unknown_uuid() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/ebooks/does-not-exist/kepub", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

/// Cache hit: a fresh KEPUB already on disk is served verbatim with a
/// `.kepub.epub` filename (no kepubify needed).
#[tokio::test]
async fn api_get_ebook_kepub_serves_cached_kepub_with_kepub_filename() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, book_id, tmp) = seed_epub_on_disk(&pool).await;

    let data_dir = DataDirGuard::new("kepub_hit");
    let cache = data_dir.path.join("kepub");
    std::fs::create_dir_all(&cache).unwrap();
    std::fs::write(cache.join(format!("{book_id}.kepub.epub")), b"KEPUB-CACHED").unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/kepub"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        content_disposition(&res),
        format!("attachment; filename=\"{uuid}.kepub.epub\""),
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"KEPUB-CACHED");

    std::fs::remove_dir_all(&tmp).ok();
}

/// Fallback: with no cache and an EPUB kepubify can't convert (invalid input
/// or binary absent), the handler serves the plain EPUB with a `.epub`
/// filename rather than erroring.
#[tokio::test]
async fn api_get_ebook_kepub_falls_back_to_plain_epub() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, _book_id, tmp) = seed_epub_on_disk(&pool).await;

    // Empty cache dir → conversion is attempted and fails on the fake EPUB.
    let _data_dir = DataDirGuard::new("kepub_fallback");

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/kepub"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        content_disposition(&res),
        format!("attachment; filename=\"{uuid}.epub\""),
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"PK\x03\x04 fake-epub");

    std::fs::remove_dir_all(&tmp).ok();
}

// -------------------------------------------------------------------
// /api/ebooks/{uuid}/download — raw EPUB download (attachment)
// -------------------------------------------------------------------

#[tokio::test]
async fn api_get_ebook_download_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/ebooks/some-uuid/download"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

/// AC1 (#1810): `can_download = 0` must 403 the raw-EPUB download route
/// before it ever resolves the uuid, mirroring `deny_without_download` on
/// the OPDS acquisition delegate for the same URL shape.
#[tokio::test]
async fn api_get_ebook_download_returns_403_when_user_cannot_download() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    revoke_can_download(&pool, user.id).await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/ebooks/some-uuid/download", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn api_get_ebook_download_returns_404_for_unknown_uuid() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer(
            "/api/ebooks/does-not-exist/download",
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_ebook_download_returns_200_with_attachment_disposition() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("omnibus_ebook_download_test_{pid}_{nanos}"));
    std::fs::create_dir_all(&tmp).unwrap();
    let stem = "alpha";
    let file_path = tmp.join(format!("{stem}.epub"));
    std::fs::write(&file_path, b"PK\x03\x04 fake-epub").unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(tmp.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "33333333-3333-3333-3333-333333333333";
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'Alpha')")
            .bind(uuid)
            .bind(lib_id)
            .bind(tmp.to_str().unwrap())
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', ?, 0)",
    )
    .bind(book_id)
    .bind(stem)
    .execute(&pool)
    .await
    .unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/download"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/epub+zip"),
    );
    let disposition = res
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default();
    assert!(
        disposition.starts_with("attachment;"),
        "download must force an attachment disposition, got {disposition:?}"
    );
    assert!(
        disposition.contains("alpha.epub"),
        "disposition should suggest the on-disk filename, got {disposition:?}"
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"PK\x03\x04 fake-epub");

    std::fs::remove_dir_all(&tmp).ok();
}

/// F5.8 #1372: a book with a metadata override downloads an EPUB whose
/// *internal* OPF carries the override — proving the route runs the source
/// through the in-place rewrite rather than shipping it verbatim.
#[tokio::test]
async fn api_get_ebook_download_bakes_metadata_override_into_epub() {
    use std::io::Cursor;

    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("omnibus_dl_override_{pid}_{nanos}"));
    let export = tmp.join("export");
    std::fs::create_dir_all(&export).unwrap();
    // Isolate the export cache so the rewrite doesn't land in ./data.
    let _env = omnibus_db::test_support::EnvVarGuard::set_os(
        "OMNIBUS_EXPORT_EPUB_DIR",
        Some(export.as_os_str()),
    );

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
    let uuid = "44444444-4444-4444-4444-444444444444";
    let book_id = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title, last_modified) \
         VALUES (?, ?, ?, 'Alpha', 1)",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(tmp.to_str().unwrap())
    .execute(&pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', ?, 0)",
    )
    .bind(book_id)
    .bind(stem)
    .execute(&pool)
    .await
    .unwrap();

    let overrides = omnibus_shared::MetadataOverrides {
        title: Some("Stormlight #1".into()),
        ..Default::default()
    };
    db::upsert_metadata_overrides(&pool, uuid, &overrides, false, user.id)
        .await
        .unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/download"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();

    let doc = epub::doc::EpubDoc::from_reader(Cursor::new(bytes.to_vec()))
        .expect("downloaded bytes are a valid EPUB");
    assert_eq!(
        doc.mdata("title").map(|m| m.value.clone()),
        Some("Stormlight #1".to_string()),
        "download must carry the baked title override"
    );

    std::fs::remove_dir_all(&tmp).ok();
}
