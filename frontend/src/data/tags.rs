//! Tag-cloud fetcher used by the discovery surface and the metadata
//! editor's tag-chip autocomplete pool.

use omnibus_shared::TagWeight;

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// GET `/api/tags` — fetch the weighted tag cloud for the discovery page.
#[cfg(feature = "mobile")]
pub async fn get_tag_cloud(server_url: &str) -> Result<Vec<TagWeight>, DataError> {
    let url = format!("{server_url}/api/tags");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<TagWeight>>().await?)
}

/// Web/SSR `get_tag_cloud` — server-function wrapper that proxies to `rpc_get_tag_cloud`.
#[cfg(not(feature = "mobile"))]
pub async fn get_tag_cloud(_server_url: &str) -> Result<Vec<TagWeight>, DataError> {
    crate::rpc::rpc_get_tag_cloud()
        .await
        .map_err(note_server_fn_err)
}
