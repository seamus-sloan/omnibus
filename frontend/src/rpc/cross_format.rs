//! Cross-format sync link RPC: the alignment read the modal renders, the
//! confirm/unlink writes that turn sync on and off, and the "synced here"
//! declaration + follow toggle. All are configuration-shaped (rule 08) —
//! called directly, never queued.

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::cross_format::{DeclareSyncPoint, SetFollowMode};
use omnibus_shared::{AlignmentView, ConfirmCrossFormatLink};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// One error mapping for every RPC in this module; the refusal variants
/// carry user-renderable messages.
#[cfg(feature = "server")]
fn rpc_error(op: &'static str, e: db::cross_format::CrossFormatError) -> ServerFnError {
    use db::cross_format::CrossFormatError as E;
    match e {
        E::BookNotFound => ServerFnError::new("book not found"),
        e @ (E::AudioSetMismatch | E::LinkRequired | E::CounterpartMissing) => {
            ServerFnError::new(e.to_string())
        }
        E::Sqlx(e) => internal_rpc_error(op, e),
    }
}

/// Alignment payload for one book: link state + staleness, both lanes'
/// raw material, and both current positions.
#[post("/api/rpc/cross-format/alignment", pool: PoolExt, user: AuthUser)]
pub async fn rpc_get_alignment(uuid: String) -> Result<AlignmentView> {
    db::cross_format::alignment_view(&pool.0, user.id, &uuid)
        .await
        .map_err(|e| rpc_error("get alignment", e).into())
}

/// Confirm (or re-confirm) the link, optionally persisting an audio
/// re-order first. Ordinals are library-wide data, so the re-order half
/// is gated on edit permission; the link itself is per-user.
#[post("/api/rpc/cross-format/link", pool: PoolExt, user: AuthUser)]
pub async fn rpc_confirm_cross_format_link(update: ConfirmCrossFormatLink) -> Result<()> {
    if let Err(msg) = update.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    if let Some(order) = &update.audio_order {
        if !user.can_edit {
            return Err(
                ServerFnError::new("reordering audio files requires edit permission").into(),
            );
        }
        if let Err(e) = db::cross_format::set_audio_order(&pool.0, &update.book_uuid, order).await {
            let msg = match &e {
                db::cross_format::CrossFormatError::AudioSetMismatch => {
                    Some(format!("{e} — reopen and retry"))
                }
                _ => None,
            };
            return Err(match msg {
                Some(msg) => ServerFnError::new(msg).into(),
                None => rpc_error("set audio order", e).into(),
            });
        }
    }
    db::cross_format::upsert_link(
        &pool.0,
        user.id,
        &update.book_uuid,
        update.mode,
        update.primary_book_file_id,
    )
    .await
    .map(|_| ())
    .map_err(|e| rpc_error("confirm cross-format link", e).into())
}

/// Turn sync off for one book; returns whether a link existed.
#[post("/api/rpc/cross-format/unlink", pool: PoolExt, user: AuthUser)]
pub async fn rpc_unlink_cross_format(uuid: String) -> Result<bool> {
    db::cross_format::delete_link(&pool.0, user.id, &uuid)
        .await
        .map_err(|e| rpc_error("unlink cross-format", e).into())
}

/// A "synced here" declaration from the reader or the player: records a
/// user anchor at the declaring surface's position and turns follow mode
/// on. Refusals (no link on a multi-file book, stale audio set, no
/// counterpart position) come back as renderable messages.
#[post("/api/rpc/cross-format/sync-point", pool: PoolExt, user: AuthUser)]
pub async fn rpc_declare_sync_point(decl: DeclareSyncPoint) -> Result<()> {
    if let Err(msg) = decl.validate() {
        return Err(ServerFnError::new(msg).into());
    }
    db::cross_format::declare_sync_point(&pool.0, user.id, &decl)
        .await
        .map(|_| ())
        .map_err(|e| rpc_error("declare sync point", e).into())
}

/// Flip follow mode on an existing link.
#[post("/api/rpc/cross-format/follow", pool: PoolExt, user: AuthUser)]
pub async fn rpc_set_follow_mode(uuid: String, body: SetFollowMode) -> Result<()> {
    match db::cross_format::set_follow(&pool.0, user.id, &uuid, body.enabled).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(ServerFnError::new("confirm the alignment first").into()),
        Err(e) => Err(rpc_error("set follow mode", e).into()),
    }
}
