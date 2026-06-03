//! `GET /api/tags` handler.
//!
//! Cookie-gated read returning the tag cloud (tag name + weight) for the
//! configured library. Powers the tag-cloud discovery page on mobile.

use axum::{
    extract::State,
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};

use super::{internal, AppState};
use crate::auth::AuthUser;

pub(super) async fn get_tags(_user: AuthUser, State(state): State<AppState>) -> Response {
    match db::get_tag_cloud(&state.pool).await {
        Ok(tags) => Json(tags).into_response(),
        Err(error) => internal("read tags", error),
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::StatusCode};
    use tower::ServiceExt;

    use super::*;
    use crate::auth::test_support as auth_test_support;
    use crate::backend::test_support::*;

    #[tokio::test]
    async fn api_get_tags_returns_200_with_tag_weights() {
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

        // Seed a book with subjects (tags).
        db::replace_books(
            &pool,
            "/lib",
            vec![db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "tagged.epub".into(),
                    title: Some("Tagged Book".into()),
                    subjects: vec!["Fiction".into(), "Science".into()],
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();

        let response = app
            .oneshot(get_with_bearer("/api/tags", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let tags: Vec<omnibus_shared::TagWeight> = serde_json::from_slice(&bytes).unwrap();
        assert!(!tags.is_empty());
        assert!(tags.iter().any(|t| t.name == "Fiction"));
        assert!(tags.iter().any(|t| t.name == "Science"));
        // Each tag should have count = 1 since we seeded one book.
        for tag in &tags {
            assert_eq!(tag.count, 1);
        }
    }

    #[tokio::test]
    async fn api_get_tags_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/tags")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}
