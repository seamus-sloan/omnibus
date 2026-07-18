use super::*;
use axum::{
    body::{to_bytes, Body},
    http::Request,
    routing::get,
};
use tower::ServiceExt;

/// One test drives all four ACs. `layer_and_route` memoizes its (layer, route)
/// so repeat calls are safe, but a single test keeps the metric counts it
/// asserts on free of cross-test bleed through the shared global recorder.
#[tokio::test]
async fn metrics_endpoint_renders_labeled_histograms_and_groups_id_paths() {
    let (layer, metrics_route) = layer_and_route();
    let app: Router = Router::new()
        .route("/api/ebooks/{uuid}", get(|| async { "ok" }))
        .merge(metrics_route)
        .layer(layer);

    // Exercise an id-bearing route twice with different ids (AC3, AC4).
    for id in ["42", "999"] {
        let res = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(format!("/api/ebooks/{id}"))
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(res.status(), 200);
    }

    let res = app
        .oneshot(
            Request::builder()
                .uri("/metrics")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(res.status(), 200);
    let body = to_bytes(res.into_body(), usize::MAX).await.unwrap();
    let text = String::from_utf8(body.to_vec()).expect("exposition text is utf8");

    // AC1: Prometheus text exposition format for HTTP request histograms.
    assert!(
        text.contains("axum_http_requests_total"),
        "expected the request counter family, got:\n{text}",
    );
    assert!(
        text.contains("axum_http_requests_duration_seconds"),
        "expected the latency histogram family, got:\n{text}",
    );

    // AC2: series labeled by method, status, and (grouped) path.
    let total = text
        .lines()
        .find(|l| {
            l.starts_with("axum_http_requests_total")
                && l.contains(r#"endpoint="/api/ebooks/{uuid}""#)
        })
        .expect("a total series for the grouped ebooks route");
    assert!(total.contains(r#"method="GET""#), "method label: {total}");
    assert!(total.contains(r#"status="200""#), "status label: {total}");

    // AC3: both requests are recorded — the grouped series counts 2.
    assert!(
        total.trim_end().ends_with(" 2"),
        "expected two requests recorded on the grouped series, got: {total}",
    );

    // AC4: raw ids never leak into labels — cardinality stays bounded.
    assert!(
        !text.contains("/api/ebooks/42") && !text.contains("/api/ebooks/999"),
        "raw ids must not appear as endpoint labels, got:\n{text}",
    );
}
