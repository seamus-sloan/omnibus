//! Discovery reads that back cross-cutting search surfaces: the tag cloud and
//! the command-palette grouped search.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
use omnibus_shared::{PaletteResults, TagWeight};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{AuthUser, PoolExt};

/// Return all tags with book counts for the tag cloud.
#[get("/api/rpc/tags", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_get_tag_cloud() -> Result<Vec<TagWeight>> {
    Ok(db::get_tag_cloud(&pool.0).await?)
}

/// Search palette — grouped results (books, authors, series, tags) for the
/// command-palette overlay.
#[post("/api/rpc/search-palette", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_search_palette(q: String) -> Result<PaletteResults> {
    let settings = db::get_settings(&pool.0).await?;
    let Some(path) = settings.ebook_library_path else {
        return Ok(PaletteResults::default());
    };
    Ok(db::search_palette(&pool.0, &path, &q).await?)
}
