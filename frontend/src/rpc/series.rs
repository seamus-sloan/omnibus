//! Series detail fetch and the series index.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
use omnibus_shared::{SeriesDetail, SeriesSummary};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Fetch a single series and all its books (ordered by series index).
#[post("/api/rpc/series", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_get_series(id: i64) -> Result<Option<SeriesDetail>> {
    Ok(db::get_series(&pool.0, id)
        .await
        .map_err(|e| internal_rpc_error("get series", e))?)
}

/// `/series` index: every series across both ebook and audiobook libraries.
#[get("/api/rpc/series-list", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_list_series() -> Result<Vec<SeriesSummary>> {
    let settings = db::get_settings(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("get settings", e))?;
    let paths = db::collect_paths(
        settings.ebook_library_path.as_deref(),
        settings.audiobook_library_path.as_deref(),
    );
    Ok(db::list_series(&pool.0, &paths)
        .await
        .map_err(|e| internal_rpc_error("list series", e))?)
}
