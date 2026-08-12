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
