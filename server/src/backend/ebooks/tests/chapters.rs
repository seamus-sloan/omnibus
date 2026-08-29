//! `GET /api/ebooks/{uuid}/chapters` and `.../chapters/{spine_index}/text`:
//! auth gating, unknown-uuid 404s, the persisted and on-the-fly listing
//! paths, the structured no-text answer, and the text read's truncation
//! boundary.

use axum::{body::to_bytes, http::StatusCode};
use omnibus_shared::{ChapterListResponse, ChapterTextResponse};
use tower::ServiceExt;

use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

use super::super::*;

/// Seed one EPUB book whose on-disk file holds `epub_bytes`. Returns
/// `(uuid, book_id, book_file_id, tmp)`; caller removes `tmp`.
async fn seed_epub_with_bytes(
    pool: &sqlx::SqlitePool,
    epub_bytes: &[u8],
) -> (String, i64, i64, std::path::PathBuf) {
    let tmp = db::test_support::make_test_dir("chapter_routes");
    std::fs::write(tmp.join("alpha.epub"), epub_bytes).unwrap();

    let lib_id = sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, 'lib')")
        .bind(tmp.to_str().unwrap())
        .execute(pool)
        .await
        .unwrap()
        .last_insert_rowid();
    let uuid = "77777777-7777-7777-7777-777777777777".to_string();
    let book_id =
        sqlx::query("INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, ?, 'Alpha')")
            .bind(&uuid)
            .bind(lib_id)
            .bind(tmp.to_str().unwrap())
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid();
    let file_id = sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'EPUB', 'alpha', 0)",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid();
    (uuid, book_id, file_id, tmp)
}

/// A two-document EPUB with a nav TOC naming the second document.
fn two_chapter_epub() -> Vec<u8> {
    db::test_support::build_test_epub_with_nav(
        &[
            (
                "c1.xhtml",
                "<html><body><p>First chapter text.</p></body></html>",
            ),
            (
                "c2.xhtml",
                "<html><body><p>Second chapter text.</p></body></html>",
            ),
        ],
        &[("Chapter Two", "c2.xhtml")],
    )
}

/// Seed a book with only a CBZ file row — the no-text shape. The archive
/// itself is never opened, so no bytes are written.
async fn seed_comic_only(pool: &sqlx::SqlitePool) -> String {
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/nowhere', 'lib')")
        .execute(pool)
        .await
        .unwrap();
    let uuid = "88888888-8888-8888-8888-888888888888".to_string();
    let book_id = sqlx::query(
        "INSERT INTO books (uuid, library_id, path, title) \
         SELECT ?, id, '/nowhere', 'Comic' FROM scan_roots LIMIT 1",
    )
    .bind(&uuid)
    .execute(pool)
    .await
    .unwrap()
    .last_insert_rowid();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) \
         VALUES (?, 'CBZ', 'comic', 0)",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap();
    uuid
}

async fn json_body<T: serde::de::DeserializeOwned>(res: axum::response::Response) -> T {
    let bytes = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn api_get_ebook_chapters_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/ebooks/some-uuid/chapters"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_ebook_chapter_text_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app
        .oneshot(get_anon("/api/ebooks/some-uuid/chapters/0/text"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn api_get_ebook_chapters_returns_404_for_unknown_uuid() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer("/api/ebooks/no-such-uuid/chapters", &token))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_ebook_chapter_text_returns_404_for_unknown_uuid() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            "/api/ebooks/no-such-uuid/chapters/0/text",
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_ebook_chapters_serves_the_persisted_structure() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    // The file's bytes are never opened on this path — only the stored rows.
    let (uuid, _, file_id, tmp) = seed_epub_with_bytes(&pool, b"not opened").await;
    let structure = db::ebook::toc::EpubStructure {
        spine: vec![
            db::ebook::toc::SpineStat {
                spine_index: 0,
                href: "c1.xhtml".into(),
                visible_chars: 40,
            },
            db::ebook::toc::SpineStat {
                spine_index: 1,
                href: "c2.xhtml".into(),
                visible_chars: 60,
            },
        ],
        chapters: vec![db::ebook::toc::TocChapter {
            ordinal: 0,
            title: "Stored Chapter".into(),
            href: "c2.xhtml".into(),
            spine_index: 1,
            start_chars: 40,
        }],
    };
    db::epub_structure::replace_structure(&pool, file_id, &structure)
        .await
        .unwrap();

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/chapters"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: ChapterListResponse = json_body(res).await;
    assert!(body.has_text);
    assert_eq!(body.book_uuid, uuid);
    assert_eq!(body.spine_count, 2);
    assert_eq!(body.chapters.len(), 1);
    assert_eq!(body.chapters[0].title, "Stored Chapter");
    assert_eq!(body.chapters[0].spine_index, 1);

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_chapters_extracts_on_the_fly_when_never_backfilled() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, _, _, tmp) = seed_epub_with_bytes(&pool, &two_chapter_epub()).await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/chapters"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: ChapterListResponse = json_body(res).await;
    assert!(body.has_text);
    assert_eq!(body.spine_count, 2);
    assert_eq!(body.chapters.len(), 1);
    assert_eq!(body.chapters[0].title, "Chapter Two");
    assert_eq!(body.chapters[0].spine_index, 1);

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_chapters_reports_no_text_for_a_comic_only_book() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_comic_only(&pool).await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/chapters"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: ChapterListResponse = json_body(res).await;
    assert!(!body.has_text);
    assert_eq!(body.spine_count, 0);
    assert!(body.chapters.is_empty());
}

#[tokio::test]
async fn api_get_ebook_chapter_text_returns_the_stripped_prose() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, _, _, tmp) = seed_epub_with_bytes(&pool, &two_chapter_epub()).await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/chapters/1/text"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: ChapterTextResponse = json_body(res).await;
    assert!(body.has_text);
    assert_eq!(body.spine_index, 1);
    assert_eq!(body.text, "Second chapter text.");
    assert_eq!(body.offset, 0);
    assert_eq!(body.total_chars, body.text.chars().count() as i64);
    assert!(!body.truncated);
    assert_eq!(body.next_offset, None);

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_chapter_text_reports_the_truncation_boundary() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, _, _, tmp) = seed_epub_with_bytes(&pool, &two_chapter_epub()).await;
    let app = crate::backend::rest_router(AppState::new(pool));

    // "First chapter text." is 19 chars; a limit of 5 forces two slices.
    let first: ChapterTextResponse = {
        let res = app
            .clone()
            .oneshot(get_with_bearer(
                &format!("/api/ebooks/{uuid}/chapters/0/text?limit=5"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        json_body(res).await
    };
    assert_eq!(first.text, "First");
    assert!(first.truncated);
    assert_eq!(first.total_chars, 19);
    assert_eq!(first.next_offset, Some(5));

    // Continuing from the reported boundary yields the rest, unmarked.
    let rest: ChapterTextResponse = {
        let res = app
            .oneshot(get_with_bearer(
                &format!("/api/ebooks/{uuid}/chapters/0/text?offset=5"),
                &token,
            ))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
        json_body(res).await
    };
    assert_eq!(rest.offset, 5);
    assert!(!rest.truncated);
    assert_eq!(rest.next_offset, None);
    assert_eq!(
        format!("{}{}", first.text, rest.text),
        "First chapter text."
    );

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_chapter_text_returns_404_for_an_out_of_range_index() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let (uuid, _, _, tmp) = seed_epub_with_bytes(&pool, &two_chapter_epub()).await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/chapters/9/text"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::NOT_FOUND);

    std::fs::remove_dir_all(&tmp).ok();
}

#[tokio::test]
async fn api_get_ebook_chapter_text_reports_no_text_for_a_comic_only_book() {
    let (_, _, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;
    let uuid = seed_comic_only(&pool).await;

    let app = crate::backend::rest_router(AppState::new(pool));
    let res = app
        .oneshot(get_with_bearer(
            &format!("/api/ebooks/{uuid}/chapters/0/text"),
            &token,
        ))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let body: ChapterTextResponse = json_body(res).await;
    assert!(!body.has_text);
    assert_eq!(body.text, "");
    assert_eq!(body.total_chars, 0);
    assert!(!body.truncated);
}
