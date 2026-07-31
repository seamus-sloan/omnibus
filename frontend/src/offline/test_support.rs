//! Shared test-only fixtures for the `offline` module tree: real `reqwest`
//! errors produced against live sockets (refused-connect for the offline
//! class, garbage-body for the decode class) so classifiers are exercised
//! on the exact error values production sees, plus a byte-exact ZIP builder
//! for the download-integrity tests.

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
    let base_url = spawn_router(app).await;
    let err = crate::data::http_client()
        .get(format!("{base_url}/j"))
        .send()
        .await
        .expect("send")
        .json::<Vec<i64>>()
        .await
        .expect_err("decode must fail");
    DataError::from(err)
}

/// Serve `app` on a loopback port and return its base URL. Shared by every
/// test in the `offline` tree that needs a real socket to drive a `reqwest`
/// call against — download resume, decode errors, batched validator sweeps.
pub(crate) async fn spawn_router(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("bind");
    let port = listener.local_addr().expect("addr").port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    format!("http://127.0.0.1:{port}")
}

/// A structurally valid ZIP holding one stored (uncompressed) entry with a
/// **correct** CRC-32, laid out exactly as an EPUB's `mimetype` member is.
///
/// The CRC has to be real: `verify` reads every member to EOF precisely to
/// check it, so a fixture with a zeroed CRC is a damaged file.
pub(crate) fn zip_one(name: &[u8], body: &[u8]) -> Vec<u8> {
    let crc = crc32(body);
    let mut out = Vec::new();

    let local_offset = out.len() as u32;
    out.extend_from_slice(b"PK\x03\x04");
    out.extend_from_slice(&[20, 0]); // version needed
    out.extend_from_slice(&[0, 0]); // flags
    out.extend_from_slice(&[0, 0]); // method: stored
    out.extend_from_slice(&[0, 0, 0, 0]); // time + date
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0, 0]); // extra len
    out.extend_from_slice(name);
    out.extend_from_slice(body);

    let cd_offset = out.len() as u32;
    out.extend_from_slice(b"PK\x01\x02");
    out.extend_from_slice(&[20, 0, 20, 0]); // version made by / needed
    out.extend_from_slice(&[0, 0]); // flags
    out.extend_from_slice(&[0, 0]); // method
    out.extend_from_slice(&[0, 0, 0, 0]); // time + date
    out.extend_from_slice(&crc.to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&(body.len() as u32).to_le_bytes());
    out.extend_from_slice(&(name.len() as u16).to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // extra / comment / disk
    out.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // attrs
    out.extend_from_slice(&local_offset.to_le_bytes());
    out.extend_from_slice(name);
    let cd_size = out.len() as u32 - cd_offset;

    out.extend_from_slice(b"PK\x05\x06");
    out.extend_from_slice(&[0, 0, 0, 0]); // disk numbers
    out.extend_from_slice(&[1, 0, 1, 0]); // entry counts
    out.extend_from_slice(&cd_size.to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&[0, 0]); // comment len
    out
}

/// Byte offset of `zip_one`'s member body — where the CRC-covered bytes
/// start, for a test that wants to corrupt one of them.
pub(crate) fn zip_one_body_offset(name: &[u8]) -> usize {
    const LOCAL_HEADER_LEN: usize = 30;
    LOCAL_HEADER_LEN + name.len()
}

/// Bit-reversed CRC-32 (the ZIP/PKZIP polynomial), computed here so the
/// fixtures don't depend on the archive crate they exist to exercise.
fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}
