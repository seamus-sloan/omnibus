//! Star-rating transport. Wraps set/get/clear for mobile (`/api/ratings*` via
//! reqwest) and web/SSR (RPC server functions in `crate::rpc`). Mobile and
//! web/SSR variants share each function's public signature so callers stay
//! platform-agnostic; the `#[cfg]` gates carry the split.

use omnibus_shared::{AttributedRating, RatingRecord, RatingUpdate};

#[cfg(not(feature = "mobile"))]
use super::note_server_fn_err;
use super::DataError;
#[cfg(feature = "mobile")]
use super::{drain_error, http_client, note_status, with_bearer};

/// POST `/api/ratings` — set or change the current user's rating.
/// Queued offline (coalesced per book, last write wins).
#[cfg(feature = "mobile")]
pub async fn set_rating(server_url: &str, update: RatingUpdate) -> Result<RatingRecord, DataError> {
    let record = crate::offline::sync::write_through(
        || set_rating_online(server_url, update.clone()),
        || crate::offline::outbox::queue_set_rating(&update),
    )
    .await?;
    crate::offline::cache::put_json(
        &crate::offline::cache::keys::rating(&record.book_uuid),
        &Some(record.clone()),
    );
    Ok(record)
}

#[cfg(feature = "mobile")]
pub(crate) async fn set_rating_online(
    server_url: &str,
    update: RatingUpdate,
) -> Result<RatingRecord, DataError> {
    let url = format!("{server_url}/api/ratings");
    let response = with_bearer(http_client().post(&url).json(&update))
        .send()
        .await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<RatingRecord>().await?)
}

/// GET `/api/ratings/{uuid}` — fetch the current rating, if any.
#[cfg(feature = "mobile")]
pub async fn get_rating(server_url: &str, uuid: &str) -> Result<Option<RatingRecord>, DataError> {
    crate::offline::cache::read_through(
        crate::offline::cache::keys::rating(uuid),
        get_rating_online(server_url, uuid),
    )
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn get_rating_online(
    server_url: &str,
    uuid: &str,
) -> Result<Option<RatingRecord>, DataError> {
    let url = format!("{server_url}/api/ratings/{uuid}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Option<RatingRecord>>().await?)
}

/// GET `/api/ratings/others/{uuid}` — list every other user's rating.
#[cfg(feature = "mobile")]
pub async fn list_other_ratings(
    server_url: &str,
    uuid: &str,
) -> Result<Vec<AttributedRating>, DataError> {
    crate::offline::cache::read_through(
        crate::offline::cache::keys::ratings_others(uuid),
        list_other_ratings_online(server_url, uuid),
    )
    .await
}

#[cfg(feature = "mobile")]
pub(crate) async fn list_other_ratings_online(
    server_url: &str,
    uuid: &str,
) -> Result<Vec<AttributedRating>, DataError> {
    let url = format!("{server_url}/api/ratings/others/{uuid}");
    let response = with_bearer(http_client().get(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(response.json::<Vec<AttributedRating>>().await?)
}

/// DELETE `/api/ratings/{uuid}` — clear (un-rate) the current rating.
#[cfg(feature = "mobile")]
pub async fn clear_rating(server_url: &str, uuid: &str) -> Result<(), DataError> {
    crate::offline::sync::write_through(
        || clear_rating_online(server_url, uuid),
        || async {
            crate::offline::outbox::queue_clear_rating(uuid)
                .await
                .then_some(())
        },
    )
    .await?;
    crate::offline::cache::put_json(
        &crate::offline::cache::keys::rating(uuid),
        &None::<omnibus_shared::RatingRecord>,
    );
    Ok(())
}

#[cfg(feature = "mobile")]
pub(crate) async fn clear_rating_online(server_url: &str, uuid: &str) -> Result<(), DataError> {
    let url = format!("{server_url}/api/ratings/{uuid}");
    let response = with_bearer(http_client().delete(&url)).send().await?;
    let status = note_status(response.status());
    if !status.is_success() {
        return Err(drain_error(response, status).await);
    }
    Ok(())
}

/// Web/SSR `set_rating` — server-function wrapper that proxies to `rpc_set_rating`.
#[cfg(not(feature = "mobile"))]
pub async fn set_rating(
    _server_url: &str,
    update: RatingUpdate,
) -> Result<RatingRecord, DataError> {
    crate::rpc::rpc_set_rating(update)
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `get_rating` — server-function wrapper that proxies to `rpc_get_rating`.
#[cfg(not(feature = "mobile"))]
pub async fn get_rating(_server_url: &str, uuid: &str) -> Result<Option<RatingRecord>, DataError> {
    crate::rpc::rpc_get_rating(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR attributed-rating list — server-function wrapper.
#[cfg(not(feature = "mobile"))]
pub async fn list_other_ratings(
    _server_url: &str,
    uuid: &str,
) -> Result<Vec<AttributedRating>, DataError> {
    crate::rpc::rpc_list_other_ratings(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}

/// Web/SSR `clear_rating` — server-function wrapper that proxies to `rpc_clear_rating`.
#[cfg(not(feature = "mobile"))]
pub async fn clear_rating(_server_url: &str, uuid: &str) -> Result<(), DataError> {
    crate::rpc::rpc_clear_rating(uuid.to_string())
        .await
        .map_err(note_server_fn_err)
}
