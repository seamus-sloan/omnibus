//! Shared test-only fixtures for the `offline` module tree: real `reqwest`
//! errors produced against live sockets (refused-connect for the offline
//! class, garbage-body for the decode class) so classifiers are exercised
//! on the exact error values production sees.

#![cfg(test)]

use crate::data::DataError;

/// A real connect-refused `DataError::Network` (port 1 is never listening).
pub(crate) async fn connect_refused_error() -> DataError {
    let err = crate::data::http_client()
        .get("http://127.0.0.1:1/nope")
        .send()
        .await
        .expect_err("connect must fail");
    DataError::from(err)
}

/// A real decode-class `DataError::Network`: the server answers 200 with a
/// body that isn't the expected JSON shape.
pub(crate) async fn decode_error() -> DataError {
    use axum::routing::get;
    let app = axum::Router::new().route("/j", get(|| async { "not-json" }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    let err = crate::data::http_client()
        .get(format!("http://127.0.0.1:{port}/j"))
        .send()
        .await
        .expect("send")
        .json::<Vec<i64>>()
        .await
        .expect_err("decode must fail");
    DataError::from(err)
}
