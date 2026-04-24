//! CSRF origin-check middleware.
//!
//! Rejects state-changing cookie-authed requests whose `Origin`/`Referer`
//! doesn't match the request's `Host`. Bearer-authed requests (mobile) are
//! exempt because browsers don't auto-attach bearer headers cross-site.
//! Safe methods (`GET`/`HEAD`/`OPTIONS`) always pass through.
//!
//! This is belt-and-braces on top of `SameSite=Lax`, which blocks classic
//! cross-site form POSTs but is inconsistent across browsers and doesn't
//! guard subdomain scenarios.

use axum::{
    extract::Request,
    http::{header, Method, StatusCode},
    middleware::Next,
    response::{IntoResponse, Response},
};
use axum_extra::extract::cookie::CookieJar;

use super::SESSION_COOKIE;

pub async fn origin_check(req: Request, next: Next) -> Response {
    let method = req.method();
    if matches!(method, &Method::GET | &Method::HEAD | &Method::OPTIONS) {
        return next.run(req).await;
    }

    // Bearer requests: exempt.
    if let Some(auth) = req.headers().get(header::AUTHORIZATION) {
        if auth
            .to_str()
            .map(|s| s.starts_with("Bearer "))
            .unwrap_or(false)
        {
            return next.run(req).await;
        }
    }

    // No cookie → not a state-changing cookie auth flow; let the normal
    // extractor 401 it if needed. Parse the jar rather than substring-matching
    // the header so unrelated cookies that merely contain our name don't
    // trigger the origin check.
    let jar = CookieJar::from_headers(req.headers());
    if jar.get(SESSION_COOKIE).is_none() {
        return next.run(req).await;
    }

    let host = req
        .headers()
        .get(header::HOST)
        .and_then(|v| v.to_str().ok());
    let origin = req
        .headers()
        .get(header::ORIGIN)
        .and_then(|v| v.to_str().ok());
    let referer = req
        .headers()
        .get(header::REFERER)
        .and_then(|v| v.to_str().ok());

    if let Some(host) = host {
        if origin_matches_host(origin, host) || origin_matches_host(referer, host) {
            return next.run(req).await;
        }
    }
    (StatusCode::FORBIDDEN, "origin mismatch").into_response()
}

fn origin_matches_host(origin: Option<&str>, host: &str) -> bool {
    let Some(origin) = origin else {
        return false;
    };
    // origin is like "http://host[:port]" or a full URL for Referer.
    // Strip scheme, then take authority up to next `/`.
    let after_scheme = origin.split_once("://").map(|(_, r)| r).unwrap_or(origin);
    let authority = after_scheme.split('/').next().unwrap_or("");
    authority == host
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{body::Body, http::Request, middleware, routing::post, Router};
    use tower::ServiceExt;

    fn guarded_router() -> Router {
        Router::new()
            .route("/api/mut", post(|| async { "ok" }))
            .layer(middleware::from_fn(origin_check))
    }

    #[tokio::test]
    async fn same_origin_post_passes() {
        let res = guarded_router()
            .oneshot(
                Request::builder()
                    .uri("/api/mut")
                    .method("POST")
                    .header(header::HOST, "localhost:3000")
                    .header(header::ORIGIN, "http://localhost:3000")
                    .header(header::COOKIE, format!("{SESSION_COOKIE}=fake"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn cross_origin_post_with_cookie_is_rejected() {
        let res = guarded_router()
            .oneshot(
                Request::builder()
                    .uri("/api/mut")
                    .method("POST")
                    .header(header::HOST, "localhost:3000")
                    .header(header::ORIGIN, "http://evil.example")
                    .header(header::COOKIE, format!("{SESSION_COOKIE}=fake"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
    }

    #[tokio::test]
    async fn bearer_requests_are_exempt() {
        let res = guarded_router()
            .oneshot(
                Request::builder()
                    .uri("/api/mut")
                    .method("POST")
                    .header(header::HOST, "localhost:3000")
                    .header(header::AUTHORIZATION, "Bearer whatever")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), StatusCode::OK);
    }
}
