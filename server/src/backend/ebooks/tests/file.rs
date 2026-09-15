//! `GET /api/ebooks/{uuid}/file`: the EPUB-then-CBZ format preference, the
//! query-token auth path, and the DB-failure 500s.

use axum::{body::to_bytes, http::StatusCode};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

use super::super::*;
use super::{seed_cbz_on_disk, seed_epub_on_disk, seed_pdf_on_disk};

// -------------------------------------------------------------------
// /api/ebooks/{uuid}/file — raw EPUB byte serving
// -------------------------------------------------------------------

#[tokio::test]
async fn api_get_ebook_file_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/ebooks/some-uuid/file"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_ebook_file_returns_404_for_unknown_uuid() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let res = app
        .oneshot(get_with_bearer("/api/ebooks/does-not-exist/file", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_ebook_file_returns_200_with_epub_bytes() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    // Write a tiny stand-in EPUB file under a temp dir whose parent is
    // recorded as `books.path` and whose stem matches `book_files.filename`.
    // pid + nanos suffix (matching `backend.rs`'s covers scratch dir) so
    // concurrent/repeat runs don't race on the same path.
    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("omnibus_ebook_file_test_{pid}_{nanos}"));
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
    let uuid = "11111111-1111-1111-1111-111111111111";
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
        .oneshot(get_with_bearer(&format!("/api/ebooks/{uuid}/file"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/epub+zip"),
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"PK\x03\x04 fake-epub");

    std::fs::remove_dir_all(&tmp).ok();
}

/// AC1 (#1810): `/file` is the in-app reader stream, not a download — it
/// must stay reachable even for a `can_download = 0` user. Only the
/// `Content-Disposition: attachment` routes (`/download`, `/kepub`) enforce
/// the flag.
#[tokio::test]
async fn api_get_ebook_file_returns_200_when_user_cannot_download() {
    let (_, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    revoke_can_download(&pool, user.id).await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, _book_id, tmp) = seed_epub_on_disk(&pool).await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(&format!("/api/ebooks/{uuid}/file"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);

    std::fs::remove_dir_all(&tmp).ok();
}

/// The `/file` stream is gated by `MediaAuthUser`, so a `?token=` query param
/// authenticates it with neither a cookie nor a bearer header — the path
/// epub.js takes from inside the mobile WebView. Mirrors the bearer 200 test
/// but authenticates purely via the query token on an otherwise-anonymous GET.
#[tokio::test]
async fn api_get_ebook_file_returns_200_with_query_token() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let pid = std::process::id();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("omnibus_ebook_file_token_test_{pid}_{nanos}"));
    std::fs::create_dir_all(&tmp).unwrap();
    let stem = "alpha";
    std::fs::write(tmp.join(format!("{stem}.epub")), b"PK\x03\x04 fake-epub").unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(tmp.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "44444444-4444-4444-4444-444444444444";
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
    // Anonymous request (no Authorization header) — the query token is the
    // only credential, exactly as an `<... src>` fetch from the WebView.
    let res = app
        .oneshot(get_anon(&format!("/api/ebooks/{uuid}/file?token={token}")))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    // The cross-origin XHR path must carry the CORS opt-in.
    assert_eq!(
        res.headers()
            .get(axum::http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok()),
        Some("*"),
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"PK\x03\x04 fake-epub");

    std::fs::remove_dir_all(&tmp).ok();
}

/// The whole-file fallback for comic-only books: with no EPUB row, `/file`
/// streams the CBZ archive itself under the comic mime — the download the
/// offline clients pull, as opposed to the per-page reads the pager makes.
#[tokio::test]
async fn api_get_ebook_file_serves_cbz_when_book_has_no_epub() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, tmp) = seed_cbz_on_disk(&pool).await;
    let archive = std::fs::read(tmp.join("alpha.cbz")).unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(&format!("/api/ebooks/{uuid}/file"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/vnd.comicbook+zip"),
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], &archive[..]);

    std::fs::remove_dir_all(&tmp).ok();
}

/// A dual-format book keeps serving the EPUB — the fallback is only for
/// books with nothing else to read, matching the pager's rule that the
/// EPUB stays the primary read.
#[tokio::test]
async fn api_get_ebook_file_prefers_epub_when_book_has_both_formats() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, tmp) = seed_cbz_on_disk(&pool).await;
    std::fs::write(tmp.join("alpha.epub"), b"PK\x03\x04 fake-epub").unwrap();
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', 'alpha', 0)",
    )
    .bind(book_id)
    .execute(&pool)
    .await
    .unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(&format!("/api/ebooks/{uuid}/file"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/epub+zip"),
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], b"PK\x03\x04 fake-epub");

    std::fs::remove_dir_all(&tmp).ok();
}

/// The last rung of the ladder: a book with neither an EPUB nor a CBZ
/// streams its PDF under the PDF mime — the bytes PDF.js range-fetches and
/// the offline clients pull whole.
#[tokio::test]
async fn api_get_ebook_file_serves_pdf_when_book_has_neither_epub_nor_cbz() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, tmp) = seed_pdf_on_disk(&pool).await;
    let pdf = std::fs::read(tmp.join("alpha.pdf")).unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(&format!("/api/ebooks/{uuid}/file"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    let header = |name: axum::http::HeaderName| {
        res.headers()
            .get(name)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    };
    assert_eq!(header(axum::http::header::CONTENT_TYPE), "application/pdf");
    assert_eq!(header(axum::http::header::ACCEPT_RANGES), "bytes");
    let etag = header(axum::http::header::ETAG);
    assert!(
        etag.starts_with('"') && etag.ends_with('"'),
        "strong entity-tag, got: {etag}"
    );
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..], &pdf[..]);

    std::fs::remove_dir_all(&tmp).ok();
}

/// CBZ outranks PDF: a book scanned both ways is a comic, and the comic
/// pipeline is the offline-verified one.
#[tokio::test]
async fn api_get_ebook_file_prefers_cbz_over_pdf_when_book_has_both() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, tmp) = seed_pdf_on_disk(&pool).await;
    let archive = db::test_support::build_stored_zip(&[("p1.jpg", b"page-one")]);
    std::fs::write(tmp.join("alpha.cbz"), &archive).unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES ((SELECT id FROM books WHERE uuid = ?), 'CBZ', 'alpha', 0)",
    )
    .bind(&uuid)
    .execute(&pool)
    .await
    .unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(&format!("/api/ebooks/{uuid}/file"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/vnd.comicbook+zip"),
    );

    std::fs::remove_dir_all(&tmp).ok();
}

/// `?file_id=` names a text edition — an EPUB or a PDF row — so a PDF row
/// resolves the way a second EPUB edition does.
#[tokio::test]
async fn api_get_ebook_file_with_file_id_serves_a_pdf_row() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, tmp) = seed_pdf_on_disk(&pool).await;
    let file_id: i64 = sqlx::query_scalar("SELECT id FROM book_files WHERE format = 'PDF'")
        .fetch_one(&pool)
        .await
        .unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/file?file_id={file_id}"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(
        res.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok()),
        Some("application/pdf"),
    );

    std::fs::remove_dir_all(&tmp).ok();
}

/// `?file_id=` exists for multi-EPUB selection, so on a comic-only book it
/// stays a 404 rather than silently resolving to the archive.
#[tokio::test]
async fn api_get_ebook_file_with_file_id_returns_404_for_comic_only_book() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, tmp) = seed_cbz_on_disk(&pool).await;
    let file_id: i64 = sqlx::query_scalar("SELECT id FROM book_files WHERE format = 'CBZ'")
        .fetch_one(&pool)
        .await
        .unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/file?file_id={file_id}"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    std::fs::remove_dir_all(&tmp).ok();
}

/// Drives the first `internal(...)` arm of `get_ebook_file`: when
/// `db::resolve_book_id_by_uuid` returns `Err`, the handler must
/// surface 500 (not 404). The induce-sqlx-error idiom is to drop
/// the `books` table out from under the live pool after seeding the
/// auth gate's `users`/`sessions` tables — the gate keeps passing,
/// the handler's first query hits "no such table: books" and the
/// response collapses into the shared `internal()` envelope. Same
/// pattern is reusable for any future 5xx test that needs to
/// isolate a single db call without a mock layer.
#[tokio::test]
async fn api_get_ebook_file_returns_500_when_resolve_book_id_by_uuid_fails() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    // FK cascade: dependents of `books` (book_files, etc.) reference
    // it, so disable FK enforcement before dropping to keep the
    // single DROP statement minimal. We don't touch `users` /
    // `sessions`, so the auth gate's `lookup_session` still works.
    // `PRAGMA foreign_keys` is per-connection in SQLite, so acquire
    // one connection and pin both statements to it — `execute(&pool)`
    // would let the PRAGMA and the DROP land on different pool
    // connections and the DROP could then trip an FK error.
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE books")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer("/api/ebooks/any-uuid/file", &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}

/// Drives the second `internal(...)` arm of `get_ebook_file`: when
/// `db::resolve_book_id_by_uuid` returns `Ok(Some(_))` but
/// `db::book_file_path` returns `Err`, the handler must surface 500.
/// Seed a book row so the first call succeeds, then drop the
/// `book_files` table so the second call's JOIN errors out.
#[tokio::test]
async fn api_get_ebook_file_returns_500_when_book_file_path_fails() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind("/lib")
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "22222222-2222-2222-2222-222222222222";
    sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'B')")
        .bind(uuid)
        .bind(lib_id)
        .bind("/lib")
        .execute(&pool)
        .await
        .unwrap();

    // FKs off because `book_file_parts` references `book_files`;
    // we want the single DROP to succeed without cascading further.
    // PRAGMA + DROP pinned to a single connection — see the sibling
    // test above for the rationale.
    let mut conn = pool.acquire().await.unwrap();
    sqlx::query("PRAGMA foreign_keys = OFF")
        .execute(&mut *conn)
        .await
        .unwrap();
    sqlx::query("DROP TABLE book_files")
        .execute(&mut *conn)
        .await
        .unwrap();
    drop(conn);

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(&format!("/api/ebooks/{uuid}/file"), &token))
        .await
        .expect("request should succeed");
    assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
}
