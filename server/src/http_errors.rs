//! Shared HTTP error-response helpers used by the `auth` and `backend`
//! routers, so the generic-500 shape is defined once rather than copied per
//! module.

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};

/// Generic 500 response that never leaks internal error details to the wire.
/// The full error is logged server-side via `tracing::error!` under
/// `context` so it remains available in structured logs; the client sees
/// only the boilerplate body.
pub fn internal<E: std::fmt::Display>(context: &'static str, e: E) -> Response {
    tracing::error!(error = %e, context = context, "internal server error");
    (StatusCode::INTERNAL_SERVER_ERROR, "internal server error").into_response()
}
