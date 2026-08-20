//! Edition-picker client wrappers for the metadata-edit page: the provider
//! catalog, the fan-out search, and the detail fetch for a selected
//! candidate. Web/SSR only — mobile has no metadata-edit surface (same split
//! as `hardcover_fetch`).

use omnibus_shared::metadata_lookup::{
    EditionSearchResponse, MetadataProvider, ProviderEdition, ProviderInfo,
};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;

/// Web/SSR: the provider catalog, which the picker reads to decide whether
/// to offer a search at all.
#[cfg(not(feature = "mobile"))]
pub async fn list_metadata_providers(_server_url: &str) -> Result<Vec<ProviderInfo>, DataError> {
    crate::rpc::rpc_metadata_providers()
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub — the metadata-edit page is a web-only surface.
#[cfg(feature = "mobile")]
pub async fn list_metadata_providers(_server_url: &str) -> Result<Vec<ProviderInfo>, DataError> {
    Ok(Vec::new())
}

/// Web/SSR: search every configured provider for `query`.
#[cfg(not(feature = "mobile"))]
pub async fn search_editions(
    _server_url: &str,
    query: &str,
) -> Result<EditionSearchResponse, DataError> {
    crate::rpc::rpc_search_editions(query.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub — see [`list_metadata_providers`].
#[cfg(feature = "mobile")]
pub async fn search_editions(
    _server_url: &str,
    _query: &str,
) -> Result<EditionSearchResponse, DataError> {
    Err(DataError::Other("edition search is web-only".into()))
}

/// Web/SSR: re-fetch one selected candidate in full. `Ok(None)` means the
/// provider no longer knows the ISBN — the caller keeps the candidate it
/// already has.
#[cfg(not(feature = "mobile"))]
pub async fn hydrate_edition(
    _server_url: &str,
    source: MetadataProvider,
    provider_ref: &str,
    isbn13: &str,
) -> Result<Option<ProviderEdition>, DataError> {
    crate::rpc::rpc_hydrate_edition(source, provider_ref.to_string(), isbn13.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Mobile stub — see [`list_metadata_providers`].
#[cfg(feature = "mobile")]
pub async fn hydrate_edition(
    _server_url: &str,
    _source: MetadataProvider,
    _provider_ref: &str,
    _isbn13: &str,
) -> Result<Option<ProviderEdition>, DataError> {
    Err(DataError::Other("edition search is web-only".into()))
}
