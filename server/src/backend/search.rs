use axum::{
    extract::{Query, State},
    response::{IntoResponse, Response},
    Json,
};
use omnibus_db::{self as db};
use serde::Deserialize;

use super::{internal, with_pagination_headers, AppState};
use crate::auth::AuthUser;

#[derive(Deserialize)]
pub(super) struct SearchQuery {
    q: String,
}

pub(super) async fn get_search(
    _user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    let Some(path) = settings.ebook_library_path else {
        // Match the `/api/ebooks` contract: even an empty result attaches
        // `X-Total-Count: 0` so clients can rely on the header always
        // being present.
        return with_pagination_headers(
            Json(omnibus_shared::EbookLibrary::default()).into_response(),
            0,
        );
    };
    let books = match db::search_books(&state.pool, &path, &params.q).await {
        Ok(b) => b,
        Err(error) => return internal("search books", error),
    };
    // Issue #81: return the *full* hit count alongside the (capped) vec
    // so clients can detect truncation via the `X-Total-Count` /
    // `X-Total-Cap` headers without changing the JSON body shape.
    let total = match db::count_search_books(&state.pool, &path, &params.q).await {
        Ok(t) => t,
        Err(error) => return internal("count search books", error),
    };
    let body = Json(omnibus_shared::EbookLibrary {
        path: Some(path),
        books,
        error: None,
    })
    .into_response();
    with_pagination_headers(body, total)
}

pub(super) async fn get_search_palette(
    _user: AuthUser,
    State(state): State<AppState>,
    Query(params): Query<SearchQuery>,
) -> Response {
    let settings = match db::get_settings(&state.pool).await {
        Ok(s) => s,
        Err(error) => return internal("read settings", error),
    };
    let Some(path) = settings.ebook_library_path else {
        return Json(omnibus_shared::PaletteResults::default()).into_response();
    };
    match db::search_palette(&state.pool, &path, &params.q).await {
        Ok(results) => Json(results).into_response(),
        Err(error) => internal("search palette", error),
    }
}

#[cfg(test)]
mod tests {
    use axum::{body::to_bytes, http::StatusCode};
    use omnibus_shared::Settings;
    use tower::ServiceExt;

    use super::*;
    use crate::auth::test_support as auth_test_support;
    use crate::backend::test_support::*;
    use crate::backend::SEARCH_RATE_LIMIT_MAX;

    #[tokio::test]
    async fn api_get_search_sets_total_count_header_with_indexed_library() {
        // Issue #81: /api/search must attach X-Total-Count on every response,
        // matching the /api/ebooks contract.
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

        db::set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/lib".into()),
                audiobook_library_path: None,
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
                        title: Some("Alpha".into()),
                        ..Default::default()
                    },
                    cover: None,
                    mtime_epoch: 0,
                    size_bytes: 0,
                },
                db::ebook::IndexedBook {
                    metadata: omnibus_shared::EbookMetadata {
                        filename: "beta.epub".into(),
                        title: Some("Beta".into()),
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
            .oneshot(get_with_bearer("/api/search?q=Alpha", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("X-Total-Count")
                .and_then(|v| v.to_str().ok()),
            Some("1"),
            "X-Total-Count must reflect the FTS match count"
        );
        assert!(
            response.headers().get("X-Total-Cap").is_none(),
            "X-Total-Cap must not be set when search results fit under the cap"
        );
    }

    #[tokio::test]
    async fn api_get_search_sets_total_count_zero_when_path_not_configured() {
        // Issue #81: the early-return path (no library configured) must
        // still attach X-Total-Count: 0 so the client can rely on the
        // header always being present.
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

        let response = app
            .oneshot(get_with_bearer("/api/search?q=anything", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(
            response
                .headers()
                .get("X-Total-Count")
                .and_then(|v| v.to_str().ok()),
            Some("0"),
            "X-Total-Count must be 0 on the no-library-configured early return"
        );
        assert!(
            response.headers().get("X-Total-Cap").is_none(),
            "X-Total-Cap must not be set on the early-return path"
        );
    }

    #[tokio::test]
    async fn api_search_returns_empty_when_path_not_configured() {
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let response = app
            .oneshot(get_with_bearer("/api/search?q=hello", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let lib: omnibus_shared::EbookLibrary = serde_json::from_slice(&bytes).unwrap();
        assert!(lib.path.is_none());
        assert!(lib.books.is_empty());
    }

    #[tokio::test]
    async fn api_search_rejects_missing_q_param() {
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let response = app
            .oneshot(get_with_bearer("/api/search", &token))
            .await
            .expect("request should succeed");
        // axum's Query extractor returns 400 for missing required fields.
        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn api_search_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app.oneshot(get_anon("/api/search?q=hello")).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    // -------------------------------------------------------------------
    // /api/search/palette — search palette (F1.5)
    // -------------------------------------------------------------------

    #[tokio::test]
    async fn api_search_palette_returns_empty_when_path_not_configured() {
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;
        let response = app
            .oneshot(get_with_bearer("/api/search/palette?q=hello", &token))
            .await
            .expect("request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let results: omnibus_shared::PaletteResults = serde_json::from_slice(&bytes).unwrap();
        assert!(results.books.is_empty());
        assert!(results.authors.is_empty());
    }

    #[tokio::test]
    async fn api_search_palette_returns_401_when_anonymous() {
        let (app, _, _) = fixture().await;
        let res = app
            .oneshot(get_anon("/api/search/palette?q=hello"))
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn api_search_palette_returns_429_after_budget_exceeded() {
        // Issue #124: /api/search/* runs four heavy FTS5 queries per request,
        // so it gets a per-IP fixed-window rate limit. The limit is set to
        // SEARCH_RATE_LIMIT_MAX requests per SEARCH_RATE_LIMIT_WINDOW; the
        // (SEARCH_RATE_LIMIT_MAX + 1)th request from the same principal must
        // be rejected with 429.
        //
        // `oneshot` requests carry no `ConnectInfo<SocketAddr>` extension, so
        // the limiter's IP fallback (`0.0.0.0`) applies — every request in
        // this test shares one bucket, which is exactly what we want.
        let (app, _state, pool) = fixture().await;
        let user = auth_test_support::create_user(&pool, "alice").await;
        let token = auth_test_support::bearer_token(&pool, user.id).await;

        for i in 0..SEARCH_RATE_LIMIT_MAX {
            let res = app
                .clone()
                .oneshot(get_with_bearer("/api/search/palette?q=hello", &token))
                .await
                .expect("request should succeed");
            assert_eq!(
                res.status(),
                StatusCode::OK,
                "request #{} (1-indexed: {}) should be within budget",
                i,
                i + 1
            );
        }

        // The (MAX+1)th request must trip the limiter.
        let over_limit = app
            .clone()
            .oneshot(get_with_bearer("/api/search/palette?q=hello", &token))
            .await
            .expect("request should succeed");
        assert_eq!(
            over_limit.status(),
            StatusCode::TOO_MANY_REQUESTS,
            "request beyond SEARCH_RATE_LIMIT_MAX must return 429",
        );
    }
}
