//! Per-user Kobo server functions for the Account settings card: list, add,
//! regenerate, and revoke wireless-sync device tokens, plus the annotation
//! down-sync opt-in. Every function is scoped to the authenticated
//! `AuthUser`, so one user's request can only touch their own state (AC4).

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::KoboDeviceView;

// Only the server-gated `rpc_create_kobo_device` body validates the name, so on
// the WASM/mobile client builds (which stub the bodies) the import is unused.
#[cfg(feature = "server")]
use omnibus_db::{self as db, kobo_devices::KoboDeviceError};
#[cfg(feature = "server")]
use omnibus_shared::kobo_device_name_invalid;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Project a db row to its re-displayable wire view.
#[cfg(feature = "server")]
fn to_view(d: db::kobo_devices::KoboDevice) -> KoboDeviceView {
    KoboDeviceView {
        id: d.id,
        name: d.name,
        token: d.token,
        created_at: d.created_at,
        last_seen_at: d.last_seen_at,
    }
}

/// Map the db error: `NotFound` (unknown id / not owned by the caller) is a
/// client-facing 4xx; everything else folds to a generic 500.
#[cfg(feature = "server")]
fn map_err(context: &'static str, e: KoboDeviceError) -> ServerFnError {
    match e {
        KoboDeviceError::NotFound => ServerFnError::new("kobo device not found"),
        other => internal_rpc_error(context, other),
    }
}

/// List the caller's registered Kobo devices.
#[post("/api/rpc/kobo/devices", pool: PoolExt, user: AuthUser)]
pub async fn rpc_list_kobo_devices() -> Result<Vec<KoboDeviceView>> {
    match db::kobo_devices::list_devices(&pool.0, user.id).await {
        Ok(list) => Ok(list.into_iter().map(to_view).collect()),
        Err(e) => Err(map_err("list kobo devices", e).into()),
    }
}

/// Register a new Kobo, minting a fresh token. Returns the created view.
#[post("/api/rpc/kobo/devices/create", pool: PoolExt, user: AuthUser)]
pub async fn rpc_create_kobo_device(name: String) -> Result<KoboDeviceView> {
    if kobo_device_name_invalid(&name) {
        return Err(ServerFnError::new("enter a device name (1–100 characters)").into());
    }
    match db::kobo_devices::create_device(&pool.0, user.id, name.trim()).await {
        Ok(dev) => Ok(to_view(dev)),
        Err(e) => Err(map_err("create kobo device", e).into()),
    }
}

/// Rotate a device's token, invalidating the prior one (AC2). Returns the
/// updated view carrying the new token.
#[post("/api/rpc/kobo/devices/regenerate", pool: PoolExt, user: AuthUser)]
pub async fn rpc_regenerate_kobo_device(id: i64) -> Result<KoboDeviceView> {
    match db::kobo_devices::regenerate_token(&pool.0, user.id, id).await {
        Ok(dev) => Ok(to_view(dev)),
        Err(e) => Err(map_err("regenerate kobo device", e).into()),
    }
}

/// Remove a registered Kobo, revoking its token.
#[post("/api/rpc/kobo/devices/revoke", pool: PoolExt, user: AuthUser)]
pub async fn rpc_revoke_kobo_device(id: i64) -> Result<()> {
    match db::kobo_devices::revoke_device(&pool.0, user.id, id).await {
        Ok(()) => Ok(()),
        Err(e) => Err(map_err("revoke kobo device", e).into()),
    }
}

/// Read the caller's "sync my annotations to Kobo" opt-in.
#[post("/api/rpc/kobo/annotation-sync", pool: PoolExt, user: AuthUser)]
pub async fn rpc_kobo_annotation_sync() -> Result<bool> {
    match db::auth::sync_annotations_to_kobo(&pool.0, user.id).await {
        Ok(v) => Ok(v),
        Err(e) => Err(internal_rpc_error("read annotation sync opt-in", e).into()),
    }
}

/// Set the caller's annotation down-sync opt-in. Enabling also kicks off
/// the backlog conversion in the background, so highlights made before the
/// opt-in reach the device on its next sync.
#[post("/api/rpc/kobo/annotation-sync/set", pool: PoolExt, user: AuthUser)]
pub async fn rpc_set_kobo_annotation_sync(enabled: bool) -> Result<()> {
    match db::auth::set_sync_annotations_to_kobo(&pool.0, user.id, enabled).await {
        Ok(()) => {
            if enabled {
                db::annotations::spawn_kobo_downsync_backlog(pool.0.clone(), user.id);
            }
            Ok(())
        }
        Err(e) => Err(internal_rpc_error("set annotation sync opt-in", e).into()),
    }
}
