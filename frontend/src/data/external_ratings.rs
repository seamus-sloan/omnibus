//! Community-rating transport. Wraps the book-detail read for mobile
//! (`/api/ebooks/{uuid}/external-ratings` via reqwest) and web/SSR (the
//! `rpc_list_external_ratings` server function).

use omnibus_shared::external_ratings::ExternalRating;

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// GET `/api/ebooks/{uuid}/external-ratings` — every provider's community
/// rating for a book.
#[cfg(feature = "mobile")]
pub async fn list_external_ratings(
    server_url: &str,
    uuid: &str,
) -> Result<Vec<ExternalRating>, DataError> {
    let url = server_url.to_string();
    let uuid = uuid.to_string();
    // Library-wide rather than per-user, so the key is deliberately outside
    // `USER_SCOPED_PREFIXES` — an account switch doesn't invalidate it.
    crate::offline::cache::read_through(
        crate::offline::cache::keys::external_ratings(&uuid),
        async move { list_external_ratings_online(&url, &uuid).await },
    )
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn list_external_ratings_online(
    server_url: &str,
    uuid: &str,
) -> Result<Vec<ExternalRating>, DataError> {
    let url = format!("{server_url}/api/ebooks/{uuid}/external-ratings");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<ExternalRating>>().await?)
}

/// Web/SSR community-rating list — server-function wrapper.
#[cfg(not(feature = "mobile"))]
pub async fn list_external_ratings(
    _server_url: &str,
    uuid: &str,
) -> Result<Vec<ExternalRating>, DataError> {
    crate::rpc::rpc_list_external_ratings(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}
