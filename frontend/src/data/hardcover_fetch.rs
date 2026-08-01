//! "Fetch from Hardcover" preview wrapper for the metadata-edit page's
//! fetch/apply panel. Web/SSR only — mobile has no metadata-edit surface
//! (mirrors `merge_books`/`book_deletion_manifest`'s web-only split in
//! `data/books/manage.rs`).

use omnibus_shared::metadata_fetch::HardcoverFetchResult;

use super::{note_server_fn_err, DataError};

/// Web/SSR: look up `uuid` on Hardcover via `rpc_fetch_hardcover_metadata`.
#[cfg(not(feature = "mobile"))]
pub async fn fetch_hardcover_metadata(
    _server_url: &str,
    uuid: &str,
) -> Result<HardcoverFetchResult, DataError> {
    crate::rpc::rpc_fetch_hardcover_metadata(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub — the metadata-edit page (and this action) is a web-only
/// surface.
#[cfg(feature = "mobile")]
pub async fn fetch_hardcover_metadata(
    _server_url: &str,
    _uuid: &str,
) -> Result<HardcoverFetchResult, DataError> {
    Err(DataError::Other("fetch from Hardcover is web-only".into()))
}

/// Web/SSR: whether a Hardcover key is configured — drives whether the
/// metadata-edit page shows the "Fetch from Hardcover" action at all.
#[cfg(not(feature = "mobile"))]
pub async fn hardcover_fetch_available(_server_url: &str) -> Result<bool, DataError> {
    crate::rpc::rpc_hardcover_fetch_available()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub — always unavailable, since the action itself is web-only.
#[cfg(feature = "mobile")]
pub async fn hardcover_fetch_available(_server_url: &str) -> Result<bool, DataError> {
    Ok(false)
}
