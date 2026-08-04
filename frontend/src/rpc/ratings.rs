//! Per-user star-rating set / fetch / clear.

use dioxus::fullstack::post;
use dioxus::prelude::*;
#[cfg(feature = "server")]
use omnibus_db as db;
use omnibus_shared::{AttributedRating, RatingRecord, RatingUpdate};

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Set (or change) the current user's star rating for a book. Mobile uses the
/// analogous REST route in `server::backend::ratings`. POST carries the
/// `RatingUpdate` body — same rationale as `rpc_save_progress`.
#[post("/api/rpc/ratings/set", pool: PoolExt, user: AuthUser)]
pub async fn rpc_set_rating(update: RatingUpdate) -> Result<RatingRecord> {
    if let Err(msg) = update.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    match db::ratings::set_rating(&pool.0, user.id, &update).await {
        Ok(rec) => Ok(rec),
        Err(db::ratings::RatingError::BookNotFound) => {
            Err(ServerFnError::new("book not found").into())
        }
        Err(db::ratings::RatingError::Sqlx(e)) => Err(internal_rpc_error("set rating", e).into()),
    }
}

/// Fetch the current user's rating for a book. Returns `Ok(None)` when the
/// book is unknown or the user has not rated it yet.
#[post("/api/rpc/ratings/get", pool: PoolExt, user: AuthUser)]
pub async fn rpc_get_rating(uuid: String) -> Result<Option<RatingRecord>> {
    Ok(db::ratings::get_rating(&pool.0, user.id, &uuid)
        .await
        .map_err(|e| internal_rpc_error("get rating", e))?)
}

/// Fetch every other user's attributed rating for a book, newest first.
#[post("/api/rpc/ratings/others", pool: PoolExt, user: AuthUser)]
pub async fn rpc_list_other_ratings(uuid: String) -> Result<Vec<AttributedRating>> {
    Ok(db::ratings::list_other_ratings(&pool.0, user.id, &uuid)
        .await
        .map_err(|e| internal_rpc_error("list other ratings", e))?)
}

/// Clear (un-rate) the current user's rating for a book. A no-op when no
/// rating exists, so it always succeeds for a known or unknown uuid.
#[post("/api/rpc/ratings/clear", pool: PoolExt, user: AuthUser)]
pub async fn rpc_clear_rating(uuid: String) -> Result<()> {
    Ok(db::ratings::delete_rating(&pool.0, user.id, &uuid)
        .await
        .map_err(|e| internal_rpc_error("clear rating", e))?)
}
