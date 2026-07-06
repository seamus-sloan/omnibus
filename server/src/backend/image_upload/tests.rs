//! Tests for the shared multipart image-validation helper.

use axum::body::{to_bytes, Body};
use axum::extract::{DefaultBodyLimit, FromRequest, Multipart};
use axum::http::{Request, StatusCode};

use super::*;

const PNG_MAGIC: &[u8] = &[0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

/// Builds a single-field `multipart/form-data` body and parses it back into
/// the `Multipart` extractor, mirroring what axum does for a real request.
async fn multipart_with_field(field_name: &str, content_type: &str, bytes: &[u8]) -> Multipart {
    const BOUNDARY: &str = "test-boundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{field_name}\"\r\nContent-Type: {content_type}\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(bytes);
    body.extend_from_slice(format!("\r\n--{BOUNDARY}--\r\n").as_bytes());

    let mut request = Request::builder()
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(body))
        .expect("request should build");
    // The production router raises axum's 2 MiB `DefaultBodyLimit` for these
    // routes (see `backend.rs`); disable it here too so this test exercises
    // `extract_validated_image`'s own 10 MiB cap, not axum's unrelated default.
    DefaultBodyLimit::disable().apply(&mut request);
    Multipart::from_request(request, &())
        .await
        .expect("multipart body should parse")
}

async fn empty_multipart() -> Multipart {
    const BOUNDARY: &str = "test-boundary";
    let request = Request::builder()
        .header(
            "content-type",
            format!("multipart/form-data; boundary={BOUNDARY}"),
        )
        .body(Body::from(format!("--{BOUNDARY}--\r\n")))
        .expect("request should build");
    Multipart::from_request(request, &())
        .await
        .expect("multipart body should parse")
}

#[tokio::test]
async fn extract_validated_image_returns_detected_mime_and_bytes_for_a_valid_png() {
    let mut multipart = multipart_with_field("cover", "image/png", PNG_MAGIC).await;
    let (mime, bytes) = extract_validated_image(&mut multipart, "cover")
        .await
        .expect("valid png should be accepted");
    assert_eq!(mime, "image/png");
    assert_eq!(&bytes[..], PNG_MAGIC);
}

#[tokio::test]
async fn extract_validated_image_returns_400_when_field_is_missing() {
    let mut multipart = empty_multipart().await;
    let response = extract_validated_image(&mut multipart, "cover")
        .await
        .expect_err("missing field should error");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body, "missing 'cover' field in multipart body");
}

#[tokio::test]
async fn extract_validated_image_returns_400_for_non_image_content_type() {
    let mut multipart = multipart_with_field("photo", "text/plain", b"not an image").await;
    let response = extract_validated_image(&mut multipart, "photo")
        .await
        .expect_err("non-image content-type should error");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body, "photo must be an image");
}

#[tokio::test]
async fn extract_validated_image_rejects_svg_content_type() {
    let mut multipart =
        multipart_with_field("cover", "image/svg+xml", b"<svg xmlns=\"...\"/>").await;
    let response = extract_validated_image(&mut multipart, "cover")
        .await
        .expect_err("svg should be rejected");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body, "SVG covers are not accepted");
}

#[tokio::test]
async fn extract_validated_image_rejects_oversized_uploads() {
    let oversized = vec![0u8; MAX_IMAGE_BYTES + 1];
    let mut multipart = multipart_with_field("cover", "image/png", &oversized).await;
    let response = extract_validated_image(&mut multipart, "cover")
        .await
        .expect_err("oversized upload should error");
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    assert_eq!(body, "cover must be under 10 MB");
}

#[tokio::test]
async fn extract_validated_image_returns_415_for_unrecognized_magic_bytes() {
    let mut multipart = multipart_with_field("photo", "image/png", b"not-a-real-png").await;
    let response = extract_validated_image(&mut multipart, "photo")
        .await
        .expect_err("unrecognized magic bytes should error");
    assert_eq!(response.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
}
