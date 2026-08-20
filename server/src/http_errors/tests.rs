//! Tests for [`super::internal`]: the shared generic-500 shape. The point of
//! the helper is that the underlying error goes to the log and never to the
//! wire, so both halves are asserted here.

use axum::body::to_bytes;

use super::internal;

/// An error whose `Display` carries a value that must not reach the response.
struct SecretError;

impl std::fmt::Display for SecretError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "connection string user:hunter2@db")
    }
}

#[tokio::test]
async fn internal_returns_a_500_with_the_generic_body() {
    let response = internal("loading books", SecretError);
    assert_eq!(
        response.status(),
        axum::http::StatusCode::INTERNAL_SERVER_ERROR
    );

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&body[..], b"internal server error");
}

#[tokio::test]
async fn internal_never_leaks_the_underlying_error_to_the_wire() {
    let response = internal("loading books", SecretError);

    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).unwrap();
    assert!(!text.contains("hunter2"), "error detail leaked: {text}");
    assert!(!text.contains("loading books"), "context leaked: {text}");
}
