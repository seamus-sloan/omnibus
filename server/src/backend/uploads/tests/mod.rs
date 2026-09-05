//! Integration tests for the "add your own books" upload endpoints, split
//! by format into the sibling modules below; the multipart request and
//! uploader fixtures they share live here. The commit happy paths run a
//! real worker scan so the indexer actually inserts the book before the
//! override is layered on top.

mod audiobook;
mod ebook;

use axum::{
    body::Body,
    http::{header::AUTHORIZATION, Request},
};

/// Build a `multipart/form-data` body. Each part is
/// `(field_name, optional_filename, content)`; a filename marks a file part.
fn multipart_body(parts: &[(&str, Option<&str>, &[u8])]) -> (String, Vec<u8>) {
    let boundary = "----omnibus-upload-test-boundary";
    let mut body: Vec<u8> = Vec::new();
    for (name, filename, content) in parts {
        body.extend_from_slice(format!("--{boundary}\r\n").as_bytes());
        match filename {
            Some(fname) => body.extend_from_slice(
                format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"{fname}\"\r\n\r\n"
                )
                .as_bytes(),
            ),
            None => body.extend_from_slice(
                format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n").as_bytes(),
            ),
        }
        body.extend_from_slice(content);
        body.extend_from_slice(b"\r\n");
    }
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());
    (format!("multipart/form-data; boundary={boundary}"), body)
}

fn post_multipart(uri: &str, token: &str, content_type: &str, body: Vec<u8>) -> Request<Body> {
    Request::builder()
        .uri(uri)
        .method("POST")
        .header("content-type", content_type)
        .header(AUTHORIZATION, format!("Bearer {token}"))
        .body(Body::from(body))
        .unwrap()
}
