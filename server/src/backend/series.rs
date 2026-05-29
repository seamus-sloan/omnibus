use axum::{
    extract::{Path, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};

use super::{internal, AppState};
use crate::auth::AuthUser;

/// F1.12 — `/series` index. Returns every series in the configured
/// library with a book count, primary author, and optional accent.
pub(super) async fn get_series(_user: AuthUser, State(state): State<AppState>) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(e) => return internal("read settings", e),
    };
    let Some(path) = settings.ebook_library_path else {
        return Json(Vec::<omnibus_shared::SeriesSummary>::new()).into_response();
    };
    match db::list_series(&state.pool, &path).await {
        Ok(series) => Json(series).into_response(),
        Err(e) => internal("list series", e),
    }
}

pub(super) async fn get_series_by_id(
    _user: AuthUser,
    State(state): State<AppState>,
    Path(id): Path<i64>,
) -> Response {
    match db::get_series(&state.pool, id).await {
        Ok(Some(series)) => Json(series).into_response(),
        Ok(None) => axum::http::StatusCode::NOT_FOUND.into_response(),
        Err(error) => internal("read series", error),
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
    async fn api_get_series_returns_200_with_detail() {
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

        // Seed a book with a series.
        db::replace_books(
            &pool,
            "/lib",
            vec![db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "series-book.epub".into(),
                    title: Some("Dune".into()),
                    series: Some("Dune Chronicles".into()),
                    series_index: Some("1".into()),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .unwrap();

        let series_id: i64 =
            sqlx::query_scalar("SELECT id FROM series WHERE name = 'Dune Chronicles'")
                .fetch_one(&pool)
                .await
                .expect("series should exist");

        let response = app
            .oneshot(get_with_bearer(&format!("/api/series/{series_id}"), &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let detail: omnibus_shared::SeriesDetail = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(detail.name, "Dune Chronicles");
        assert_eq!(detail.book_count, 1);
        assert_eq!(detail.books.len(), 1);
        assert_eq!(detail.books[0].title.as_deref(), Some("Dune"));
    }

    #[tokio::test]
    async fn api_get_series_returns_404_for_unknown_id() {
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

        let response = app
            .oneshot(get_with_bearer("/api/series/9999", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn api_get_series_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/series/1")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    // -------------------------------------------------------------------
    // F1.12 — /api/series index endpoint.
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_get_series_index_returns_summaries_with_primary_author() {
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

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
            vec![db::ebook::IndexedBook {
                metadata: omnibus_shared::EbookMetadata {
                    filename: "dune.epub".into(),
                    title: Some("Dune".into()),
                    series: Some("Dune Chronicles".into()),
                    series_index: Some("1".into()),
                    creators: vec![omnibus_shared::Contributor {
                        name: "Frank Herbert".into(),
                        ..Default::default()
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

        let response = app
            .oneshot(get_with_bearer("/api/series", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);

        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let series: Vec<omnibus_shared::SeriesSummary> = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(series.len(), 1);
        assert_eq!(series[0].name, "Dune Chronicles");
        assert_eq!(series[0].book_count, 1);
        assert_eq!(series[0].primary_author.as_deref(), Some("Frank Herbert"));
    }

    #[tokio::test]
    async fn api_get_series_index_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/series")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }
}
