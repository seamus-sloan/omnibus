//! "Fetch from Hardcover" preview — the metadata-edit page's read-only
//! lookup backing the preview/apply panel. Returns candidate fields without
//! writing anything; applying them goes through the existing
//! `rpc_save_overrides` path once the user picks which fields to keep.

use dioxus::fullstack::{get, post};
use dioxus::prelude::*;
use omnibus_shared::metadata_fetch::HardcoverFetchResult;

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Look up `uuid` on Hardcover and return its canonical metadata for
/// preview. Requires `can_edit` or admin (same gate as `rpc_save_overrides`,
/// since this feeds that action). `NotConfigured` / `NotFound` are not
/// errors — see [`omnibus_shared::metadata_fetch::HardcoverFetchResult`].
#[post("/api/rpc/ebook/hardcover-fetch", pool: PoolExt, user: AuthUser)]
pub async fn rpc_fetch_hardcover_metadata(uuid: String) -> Result<HardcoverFetchResult> {
    if !user.is_admin && !user.can_edit {
        return Err(ServerFnError::new("forbidden: edit permission required").into());
    }
    Ok(db::fetch_hardcover_metadata(&pool.0, &uuid)
        .await
        .map_err(|e| internal_rpc_error("fetch hardcover metadata", e))?)
}

/// Whether a Hardcover key is configured — drives the metadata-edit page's
/// decision to show the "Fetch from Hardcover" action at all, mirroring
/// `rpc_summary_sources`'s reasoning: this is a non-sensitive yes/no (never
/// the key itself), so any authenticated user may read it, not just admins.
#[get("/api/rpc/ebook/hardcover-fetch/available", pool: PoolExt, _user: AuthUser)]
pub async fn rpc_hardcover_fetch_available() -> Result<bool> {
    Ok(db::effective_hardcover_api_key(&pool.0)
        .await
        .map_err(|e| internal_rpc_error("hardcover key status", e))?
        .is_some())
}
