//! Alignment + link transport. Web/SSR goes straight to the RPCs — these
//! writes are configuration-shaped (rule 08) and never queue. The hybrid
//! mobile shell has no alignment surface, so its arms are deliberate
//! error stubs, mirroring `merge_books`.

use omnibus_shared::{AlignmentView, ConfirmCrossFormatLink};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;

/// Fetch the alignment payload for one book.
#[cfg(not(feature = "mobile"))]
pub async fn get_alignment(_server_url: &str, uuid: &str) -> Result<AlignmentView, DataError> {
    crate::rpc::rpc_get_alignment(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Confirm (or re-confirm) the cross-format link.
#[cfg(not(feature = "mobile"))]
pub async fn confirm_cross_format_link(
    _server_url: &str,
    update: ConfirmCrossFormatLink,
) -> Result<(), DataError> {
    crate::rpc::rpc_confirm_cross_format_link(update)
        .await
        .map_err(note_server_fn_err)
}

/// Turn sync off for one book.
#[cfg(not(feature = "mobile"))]
pub async fn unlink_cross_format(_server_url: &str, uuid: &str) -> Result<bool, DataError> {
    crate::rpc::rpc_unlink_cross_format(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// The hybrid shell has no alignment surface — native iOS owns it there.
#[cfg(feature = "mobile")]
pub async fn get_alignment(_server_url: &str, _uuid: &str) -> Result<AlignmentView, DataError> {
    Err(DataError::Other(
        "alignment is not available in the mobile shell".into(),
    ))
}

#[cfg(feature = "mobile")]
pub async fn confirm_cross_format_link(
    _server_url: &str,
    _update: ConfirmCrossFormatLink,
) -> Result<(), DataError> {
    Err(DataError::Other(
        "alignment is not available in the mobile shell".into(),
    ))
}

#[cfg(feature = "mobile")]
pub async fn unlink_cross_format(_server_url: &str, _uuid: &str) -> Result<bool, DataError> {
    Err(DataError::Other(
        "alignment is not available in the mobile shell".into(),
    ))
}

/// Why a "synced here" declaration failed, split so the surfaces can
/// route "link first" to the link affordance instead of a dead-end
/// generic failure.
#[derive(Debug)]
pub enum SyncPointError {
    /// No link yet and the book can't auto-link (several audio files) —
    /// the fix is confirming the alignment, not retrying.
    LinkRequired,
    /// Genuine failure: offline, server error, anything else.
    Other(DataError),
}

/// Prefix of `CrossFormatError::LinkRequired`'s `#[error]` message
/// (`db/src/cross_format.rs`) — the refusal's de-facto wire contract, the
/// same one `rpc_set_follow_mode` relies on. The frontend can't import the
/// db crate on web/mobile builds, so the string is mirrored here.
#[cfg(not(feature = "mobile"))]
const LINK_REQUIRED_PREFIX: &str = "confirm the alignment first";

/// The `LinkRequired` refusal travels as its `thiserror` message.
#[cfg(not(feature = "mobile"))]
fn classify_sync_point_err(e: DataError) -> SyncPointError {
    match &e {
        DataError::Other(msg) if msg.starts_with(LINK_REQUIRED_PREFIX) => {
            SyncPointError::LinkRequired
        }
        _ => SyncPointError::Other(e),
    }
}

/// Declare a "synced here" sync point from the reader or player.
#[cfg(not(feature = "mobile"))]
pub async fn declare_sync_point(
    _server_url: &str,
    decl: omnibus_shared::cross_format::DeclareSyncPoint,
) -> Result<(), SyncPointError> {
    crate::rpc::rpc_declare_sync_point(decl)
        .await
        .map_err(|e| classify_sync_point_err(note_server_fn_err(e)))
}

/// Flip follow mode on an existing link.
#[cfg(not(feature = "mobile"))]
pub async fn set_follow_mode(
    _server_url: &str,
    uuid: &str,
    enabled: bool,
) -> Result<(), DataError> {
    crate::rpc::rpc_set_follow_mode(
        uuid.to_string(),
        omnibus_shared::cross_format::SetFollowMode { enabled },
    )
    .await
    .map_err(note_server_fn_err)
}

#[cfg(feature = "mobile")]
pub async fn declare_sync_point(
    _server_url: &str,
    _decl: omnibus_shared::cross_format::DeclareSyncPoint,
) -> Result<(), SyncPointError> {
    Err(SyncPointError::Other(DataError::Other(
        "alignment is not available in the mobile shell".into(),
    )))
}

#[cfg(feature = "mobile")]
pub async fn set_follow_mode(
    _server_url: &str,
    _uuid: &str,
    _enabled: bool,
) -> Result<(), DataError> {
    Err(DataError::Other(
        "alignment is not available in the mobile shell".into(),
    ))
}

#[cfg(all(test, not(feature = "mobile")))]
mod tests {
    use super::*;

    #[test]
    fn classify_sync_point_err_maps_link_refusal_to_link_required() {
        let e = DataError::Other(
            "confirm the alignment first — this book has several audio files".into(),
        );
        assert!(matches!(
            classify_sync_point_err(e),
            SyncPointError::LinkRequired
        ));
    }

    #[test]
    fn classify_sync_point_err_passes_other_failures_through() {
        let e = DataError::Other("connection reset".into());
        assert!(matches!(
            classify_sync_point_err(e),
            SyncPointError::Other(_)
        ));
    }
}
