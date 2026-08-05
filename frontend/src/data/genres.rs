//! Genre-cloud fetcher backing the genre-chip autocomplete pools on the
//! metadata editor, the landing table's inline cell, and the bulk-edit modal.

use omnibus_shared::GenreWeight;

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// GET `/api/genres` — fetch the weighted genre cloud.
#[cfg(feature = "mobile")]
pub async fn get_genre_cloud(server_url: &str) -> Result<Vec<GenreWeight>, DataError> {
    let url = server_url.to_string();
    crate::offline::cache::read_through(crate::offline::cache::keys::genres(), async move {
        get_genre_cloud_online(&url).await
    })
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn get_genre_cloud_online(
    server_url: &str,
) -> Result<Vec<GenreWeight>, DataError> {
    let url = format!("{server_url}/api/genres");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<GenreWeight>>().await?)
}

/// Web/SSR `get_genre_cloud` — server-function wrapper that proxies to
/// `rpc_get_genre_cloud`.
#[cfg(not(feature = "mobile"))]
pub async fn get_genre_cloud(_server_url: &str) -> Result<Vec<GenreWeight>, DataError> {
    crate::rpc::rpc_get_genre_cloud()
        .await
        .map_err(note_server_fn_err)
}
