//! On-demand summary-fetch wrappers for the "Fetch Summary" button. Web/SSR
//! calls the server functions; mobile ships no summary UI, so its variants are
//! inert stubs that keep the shared call sites compiling under
//! `--features mobile` (mirroring `get_manifest`'s web-only fallback).

use omnibus_shared::summary::SummarySource;

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;

/// Web/SSR: fetch a summary for `uuid` from `source` via `rpc_fetch_summary`.
/// `Ok(None)` when that source had no summary (the caller cascades to the next).
#[cfg(not(feature = "mobile"))]
pub async fn fetch_summary(
    _server_url: &str,
    uuid: &str,
    source: SummarySource,
) -> Result<Option<String>, DataError> {
    crate::rpc::rpc_fetch_summary(uuid.to_string(), source)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR: whether a server-wide Hardcover key is configured — drives whether
/// the client's cascade starts at Hardcover or goes straight to OpenLibrary.
#[cfg(not(feature = "mobile"))]
pub async fn hardcover_configured(_server_url: &str) -> Result<bool, DataError> {
    crate::rpc::rpc_hardcover_configured()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile: no summary-fetch UI ships on mobile; this stub keeps shared call
/// sites compiling.
#[cfg(feature = "mobile")]
pub async fn fetch_summary(
    _server_url: &str,
    _uuid: &str,
    _source: SummarySource,
) -> Result<Option<String>, DataError> {
    Err(DataError::Other("fetch summary is web-only".into()))
}

/// Mobile counterpart to the web `hardcover_configured`; see [`fetch_summary`].
#[cfg(feature = "mobile")]
pub async fn hardcover_configured(_server_url: &str) -> Result<bool, DataError> {
    Err(DataError::Other("fetch summary is web-only".into()))
}
