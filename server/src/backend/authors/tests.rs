//! Tests for author listing and detail handlers.
use axum::{body::to_bytes, http::StatusCode};
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

#[tokio::test]
async fn api_get_author_returns_200_with_detail() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    // Seed a book with an author via replace_books.
    db::replace_books(
        &pool,
        "/lib",
        vec![db::ebook::IndexedBook {
            metadata: omnibus_shared::EbookMetadata {
                filename: "test.epub".into(),
                title: Some("Test Book".into()),
                creators: vec![omnibus_shared::Contributor {
                    name: "Jane Austen".into(),
                    role: Some("aut".into()),
                    file_as: Some("Austen, Jane".into()),
                    id: None,
                }],
                ..Default::default()
            },
            cover: None,
            mtime_epoch: 0,
            size_bytes: 0,
        }],
    )
    .await
    .unwrap();

    // Look up the author id from the DB.
    let author_id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = 'Jane Austen'")
        .fetch_one(&pool)
        .await
        .expect("author should exist");

    let response = app
        .oneshot(get_with_bearer(
            &format!("/api/authors/{author_id}"),
            &token,
        ))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let detail: omnibus_shared::AuthorDetail = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(detail.name, "Jane Austen");
    assert_eq!(detail.book_count, 1);
    assert_eq!(detail.books.len(), 1);
    assert_eq!(detail.books[0].title.as_deref(), Some("Test Book"));
}

#[tokio::test]
async fn api_get_author_returns_404_for_unknown_id() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(get_with_bearer("/api/authors/9999", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn api_get_author_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(get_anon("/api/authors/1")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}

// -------------------------------------------------------------------
// F1.12 — /api/authors index endpoint.
// -------------------------------------------------------------------

#[tokio::test]
async fn api_get_authors_index_returns_summaries_scoped_to_library() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    // Point settings at /lib and seed two books with distinct authors.
    db::set_settings(
        &pool,
        &omnibus_shared::Settings {
            ebook_library_path: Some("/lib".into()),
            ..Default::default()
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
                    filename: "a.epub".into(),
                    title: Some("A".into()),
                    creators: vec![omnibus_shared::Contributor {
                        name: "Aaron Albright".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            },
            db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "b.epub".into(),
                    title: Some("B".into()),
                    creators: vec![omnibus_shared::Contributor {
                        name: "Zelda Zinn".into(),
                        ..Default::default()
                    }],
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            },
        ],
    )
    .await
    .unwrap();

    let response = app
        .oneshot(get_with_bearer("/api/authors", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let authors: Vec<omnibus_shared::AuthorSummary> = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(authors.len(), 2);
    let names: Vec<_> = authors.iter().map(|a| a.name.clone()).collect();
    assert_eq!(
        names,
        vec!["Aaron Albright".to_string(), "Zelda Zinn".to_string()],
        "expected alpha order"
    );
    for a in &authors {
        assert_eq!(a.book_count, 1);
    }
}

#[tokio::test]
async fn api_get_authors_index_returns_empty_when_no_library_configured() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    let response = app
        .oneshot(get_with_bearer("/api/authors", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let authors: Vec<omnibus_shared::AuthorSummary> = serde_json::from_slice(&bytes).unwrap();
    assert!(authors.is_empty());
}

#[tokio::test]
async fn api_get_authors_index_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(get_anon("/api/authors")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
