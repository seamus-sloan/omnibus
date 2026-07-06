//! Shared multipart image-upload validation: content-type check, SVG
//! rejection, size cap, and magic-byte sniff. Used by the cover-upload
//! (`overrides::post_ebook_cover`) and author-photo-upload
//! (`author_photos::put_author_photo`) handlers so the two pipelines can't
//! drift out of sync.

use axum::body::Bytes;
use axum::response::{IntoResponse, Response};
use omnibus_shared::detect_image_format;

use super::internal;

const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;

/// Reads the `field_name` field out of `multipart`, validating content-type,
/// rejecting SVG, capping size at 10 MiB, and sniffing magic bytes. Returns
/// the detected MIME + bytes on success, or the exact error `Response` the
/// caller should return on failure.
pub(super) async fn extract_validated_image(
    multipart: &mut axum::extract::Multipart,
    field_name: &str,
) -> Result<(String, Bytes), Response> {
    loop {
        match multipart.next_field().await {
            Ok(Some(field)) => {
                let name = field.name().unwrap_or("").to_string();
                if name != field_name {
                    continue;
                }
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_string();
                if !content_type.starts_with("image/") {
                    return Err((
                        axum::http::StatusCode::BAD_REQUEST,
                        format!("{field_name} must be an image"),
                    )
                        .into_response());
                }
                // Reject SVG — contains executable content and can XSS when
                // opened directly in a browser tab.
                if content_type.contains("svg") {
                    return Err((
                        axum::http::StatusCode::BAD_REQUEST,
                        format!("SVG {field_name}s are not accepted"),
                    )
                        .into_response());
                }
                match field.bytes().await {
                    Ok(b) => {
                        if b.len() > MAX_IMAGE_BYTES {
                            return Err((
                                axum::http::StatusCode::BAD_REQUEST,
                                format!("{field_name} must be under 10 MB"),
                            )
                                .into_response());
                        }
                        // Validate magic bytes — don't trust Content-Type alone.
                        // Bind the detected MIME directly: a `None` here means the
                        // bytes carry no recognisable image header, so surface a
                        // 415 rather than `.unwrap()`-panicking the task (#210).
                        match detect_image_format(&b) {
                            // Use the detected MIME so the stored extension matches
                            // actual content, not the (untrusted) client header.
                            Some(mime) => return Ok((mime, b)),
                            None => {
                                return Err((
                                    axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE,
                                    "Could not detect image format",
                                )
                                    .into_response());
                            }
                        }
                    }
                    Err(e) => return Err(internal("read image field", e)),
                }
            }
            Ok(None) => {
                return Err((
                    axum::http::StatusCode::BAD_REQUEST,
                    format!("missing '{field_name}' field in multipart body"),
                )
                    .into_response())
            }
            Err(e) => return Err(internal("parse multipart", e)),
        }
    }
}

#[cfg(test)]
mod tests;
