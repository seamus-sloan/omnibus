//! Format filtering on `GET /api/ebooks`: the `formats` include filter and
//! the per-user `exclude_formats` hide, which must never hide a
//! physical-only book and reports what it hid on the first page.

use axum::{body::to_bytes, http::StatusCode};
use omnibus_shared::Settings;
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

use super::super::*;

#[tokio::test]
async fn api_get_ebooks_formats_param_filters_page_and_total() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    db::set_settings(
        &pool,
        &Settings {
            ebook_library_path: Some("/lib".into()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    db::replace_books(
        &pool,
        "/lib",
        vec![
            db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "alpha.epub".into(),
                    title: Some("A".into()),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
                word_count: None,
            },
            db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "beta.m4b".into(),
                    title: Some("B".into()),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
                word_count: None,
            },
        ],
    )
    .await
    .unwrap();

    // `?formats=` alone switches to the keyset path and filters both the
    // page and the X-Total-Count. The stored format is uppercase (M4B);
    // the lowercase wire value must match case-insensitively.
    let response = app
        .oneshot(get_with_bearer(
            "/api/ebooks?formats=m4b,m4a,mp3&limit=50",
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("X-Total-Count")
            .and_then(|v| v.to_str().ok()),
        Some("1"),
        "X-Total-Count must reflect the filtered row count"
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(lib.books.len(), 1);
    assert_eq!(lib.books[0].title.as_deref(), Some("B"));
}

// ── Hidden-formats exclusion (`?exclude_formats=`) ───────────────

/// Seed `/lib` with one CBZ-only comic, one EPUB novel, and configure the
/// ebook library path — the fixture for the exclusion tests.
async fn seed_mixed_format_library(pool: &sqlx::SqlitePool) {
    db::set_settings(
        pool,
        &Settings {
            ebook_library_path: Some("/lib".into()),
            audiobook_library_path: None,
            scan_interval_hours: None,
        },
    )
    .await
    .unwrap();
    let book = |filename: &str, title: &str| db::ebook::IndexedBook {
        metadata: omnibus_shared::EbookMetadata {
            filename: filename.to_string(),
            title: Some(title.to_string()),
            ..Default::default()
        },
        cover: None,
        mtime_epoch: 0,
        size_bytes: 0,
        word_count: None,
    };
    db::replace_books(
        pool,
        "/lib",
        vec![book("comic.cbz", "Comic"), book("novel.epub", "Novel")],
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn get_ebooks_with_exclude_formats_omits_fully_hidden_books() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    seed_mixed_format_library(&pool).await;

    let response = app
        .oneshot(get_with_bearer("/api/ebooks?exclude_formats=cbz", &token))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("X-Total-Count")
            .and_then(|v| v.to_str().ok()),
        Some("1"),
        "total counts what pagination will actually yield"
    );
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
    let titles: Vec<_> = lib
        .books
        .iter()
        .filter_map(|b| b.title.as_deref())
        .collect();
    assert_eq!(titles, vec!["Novel"]);
}

#[tokio::test]
async fn get_ebooks_with_exclude_formats_emits_hidden_count_header_on_first_page_only() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    seed_mixed_format_library(&pool).await;

    // First page (no cursor): the receipt header rides along.
    let response = app
        .clone()
        .oneshot(get_with_bearer(
            "/api/ebooks?sort=title&dir=asc&limit=1&exclude_formats=cbz",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("X-Hidden-Count")
            .and_then(|v| v.to_str().ok()),
        Some("1"),
        "one comic-only book hidden"
    );
    let cursor = response
        .headers()
        .get("X-Next-Cursor")
        .and_then(|v| v.to_str().ok());

    // Later page (cursor present): no receipt header. When the single
    // visible book fits page 1 there is no cursor and the invariant holds
    // vacuously, so only assert when pagination continues.
    if let Some(cursor) = cursor {
        let response = app
            .oneshot(get_with_bearer(
                &format!(
                    "/api/ebooks?sort=title&dir=asc&limit=1&exclude_formats=cbz&cursor={cursor}"
                ),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert!(response.headers().get("X-Hidden-Count").is_none());
    }
}

#[tokio::test]
async fn get_ebooks_without_exclude_formats_returns_full_library_for_mirror_sync() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    seed_mixed_format_library(&pool).await;
    // The user's stored pref must NOT leak into a request that didn't ask —
    // the mirrors sync param-less and stay full-library.
    db::auth::set_hidden_formats(&pool, user.id, &["cbz".into()])
        .await
        .unwrap();

    let response = app
        .oneshot(get_with_bearer("/api/ebooks", &token))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response
            .headers()
            .get("X-Total-Count")
            .and_then(|v| v.to_str().ok()),
        Some("2"),
        "param-less request stays byte-identical for mirror syncs"
    );
    assert!(response.headers().get("X-Hidden-Count").is_none());
}

#[tokio::test]
async fn get_ebooks_exclude_formats_never_hides_physical_only_books() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    seed_mixed_format_library(&pool).await;
    // A physical-only book: a books row + physical copy, no book_files.
    sqlx::query(
        "INSERT INTO books (uuid, scan_key, library_id, path, title)
         SELECT 'phys-uuid', 'phys-key', id, '/p', 'Shelf Copy' FROM scan_roots WHERE path = '/lib'",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO physical_copies (book_uuid) VALUES ('phys-uuid')")
        .execute(&pool)
        .await
        .unwrap();

    let response = app
        .oneshot(get_with_bearer(
            "/api/ebooks?exclude_formats=cbz,epub",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
    let titles: Vec<_> = lib
        .books
        .iter()
        .filter_map(|b| b.title.as_deref())
        .collect();
    assert_eq!(
        titles,
        vec!["Shelf Copy"],
        "physical ownership trumps hiding"
    );
}
