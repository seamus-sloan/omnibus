//! Shared HTTP error-response helpers used by the `auth` and `backend`
//! routers, so the generic-500 shape is defined once rather than copied per
//! module.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[cfg(test)]
mod tests;

/// Generic 500 response that logs `e` under `context` but never leaks it to the wire.
pub(crate) fn internal<E: std::fmt::Display>(context: &'static str, e: E) -> Response {
    tracing::error!(error = %e, context = context, "internal server error");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
}
