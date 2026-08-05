//! Tests for the genre cloud handler.
use axum::{body::to_bytes, http::StatusCode};
use tower::ServiceExt;

use super::*;
use crate::auth::test_support as auth_test_support;
use crate::backend::test_support::*;

/// Seed one book and give it a genres override — the only way a genre is
/// ever created (migration `0066`).
async fn seed_book_with_genres(pool: &sqlx::SqlitePool, genres: &[&str], user_id: i64) {
    db::replace_books(
        pool,
        "/lib",
        vec![db::ebook::IndexedBook {
            metadata: omnibus_shared::EbookMetadata {
                filename: "genred.epub".into(),
                title: Some("Genred Book".into()),
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

    let books = db::list_books(pool, "/lib").await.unwrap();
    let uuid = books[0].unique_identifier.clone().unwrap();
    db::merge_metadata_overrides(
        pool,
        &uuid,
        &omnibus_shared::MetadataOverrides {
            genres: Some(genres.iter().map(|g| (*g).to_string()).collect()),
            ..Default::default()
        },
        user_id,
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn api_get_genres_returns_200_with_genre_weights() {
    let (app, _state, pool) = fixture().await;
    let user = auth_test_support::create_user(&pool, "alice").await;
    let token = auth_test_support::bearer_token(&pool, user.id).await;

    seed_book_with_genres(&pool, &["Horror", "Mystery"], user.id).await;

    let response = app
        .oneshot(get_with_bearer("/api/genres", &token))
        .await
        .expect("request should succeed");
    assert_eq!(response.status(), StatusCode::OK);

    let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let genres: Vec<omnibus_shared::GenreWeight> = serde_json::from_slice(&bytes).unwrap();
    assert!(genres.iter().any(|g| g.name == "Horror"));
    assert!(genres.iter().any(|g| g.name == "Mystery"));
    for genre in &genres {
        assert_eq!(genre.count, 1, "one seeded book per genre");
    }
}

#[tokio::test]
async fn api_get_genres_returns_401_when_anonymous() {
    let (app, _, _) = fixture().await;
    let res = app.oneshot(get_anon("/api/genres")).await.unwrap();
    assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
}
