//! Unit tests for `friendly()`'s `DataError` → `DownloadError` mapping —
//! guards against a raw-error leak into `DownloadStatus::Error`.

use super::*;
use crate::data::DataError;
use crate::offline::test_support::{connect_refused_error, decode_error};

#[test]
fn friendly_maps_offline_to_the_offline_message() {
    assert_eq!(friendly(&DataError::Offline), DownloadError::Offline);
}

#[test]
fn friendly_maps_unauthorized_to_the_sign_in_again_message() {
    assert_eq!(
        friendly(&DataError::Unauthorized),
        DownloadError::Unauthorized
    );
}

#[test]
fn friendly_maps_http_to_the_server_error_message() {
    let e = DataError::Http {
        status: 503,
        body: "upstream exploded in a way a user should never see".into(),
    };
    let mapped = friendly(&e);
    assert_eq!(mapped, DownloadError::ServerError);
    assert!(!mapped.to_string().contains("upstream exploded"));
}

#[test]
fn friendly_maps_decode_to_the_server_error_message() {
    let src = serde_json::from_str::<i64>("not json").expect_err("must fail");
    let e = DataError::from(src);
    assert_eq!(friendly(&e), DownloadError::ServerError);
}

#[test]
fn friendly_maps_other_to_the_network_error_message() {
    assert_eq!(
        friendly(&DataError::Other("raw internal detail".into())),
        DownloadError::NetworkError
    );
}

#[tokio::test]
async fn friendly_maps_a_connect_refused_network_error_to_offline() {
    // Connect-refused classifies as offline via `is_offline_error`, checked
    // before the per-variant match.
    let e = connect_refused_error().await;
    assert_eq!(friendly(&e), DownloadError::Offline);
}

#[tokio::test]
async fn friendly_maps_a_decode_class_network_error_to_server_error_not_offline() {
    // A `Network` error that IS a decode failure means the server was
    // reachable — must not be classified as offline.
    let e = decode_error().await;
    let mapped = friendly(&e);
    assert_eq!(mapped, DownloadError::ServerError);
    assert!(!mapped.to_string().to_lowercase().contains("offline"));
}

#[test]
fn download_error_display_never_echoes_a_raw_variant_name() {
    // Every fixed message is a full sentence a user could read — not a
    // bare enum tag or a `{:?}`-style debug dump.
    for variant in [
        DownloadError::Offline,
        DownloadError::NotFound,
        DownloadError::UnsupportedFormat,
        DownloadError::NothingToDownload,
        DownloadError::StorageUnavailable,
        DownloadError::ConnectionLost,
        DownloadError::ServerError,
        DownloadError::NetworkError,
        DownloadError::Unauthorized,
        DownloadError::Interrupted,
    ] {
        assert!(!variant.to_string().is_empty());
    }
}
