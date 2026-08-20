//! Community ratings from the external metadata providers — the read the
//! book-detail card renders beside the reader's own star rating.

use dioxus::fullstack::post;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use omnibus_db as db;
use omnibus_shared::external_ratings::ExternalRating;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Every community rating stored for a book. `Ok(vec![])` for a book with
/// none — and for an unknown uuid, matching the REST route's semantics. The
/// refresh that writes these lives on `POST /api/ebooks/{uuid}/external-ratings`.
#[post("/api/rpc/external-ratings/list", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_list_external_ratings(uuid: String) -> Result<Vec<ExternalRating>> {
    Ok(db::external_ratings::list_ratings(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("list external ratings", e))?)
}
