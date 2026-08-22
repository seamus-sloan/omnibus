//! Shared HTTP error-response helpers used by the `auth` and `backend`
//! routers, so the generic-500 shape is defined once rather than copied per
//! module.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

/// Generic 500 response that logs `e` under `context` but never leaks it to the wire.
pub(crate) fn internal<E: std::fmt::Display>(context: &'static str, e: E) -> Response {
    tracing::error!(error = %e, context = context, "internal server error");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
}

#[cfg(test)]
mod tests {
    use axum::body::to_bytes;

    use super::*;

    #[tokio::test]
    async fn internal_returns_a_generic_500_without_leaking_the_error() {
        let res = internal("book lookup", "connection refused: 10.0.0.5:5432");
        assert_eq!(res.status(), StatusCode::INTERNAL_SERVER_ERROR);
        let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
        let text = String::from_utf8(body.to_vec()).unwrap();
        assert_eq!(text, "internal server error");
        assert!(!text.contains("10.0.0.5"));
    }
}
