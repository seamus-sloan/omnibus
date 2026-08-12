//! Cross-format sync link RPC: the alignment read the modal renders plus
//! the confirm/unlink writes that turn sync on and off. All three are
//! configuration-shaped (rule 08) — called directly, never queued.

use dioxus::fullstack::post;
use dioxus::prelude::*;
use omnibus_shared::{AlignmentView, ConfirmCrossFormatLink};

#[cfg(feature = "server")]
use omnibus_db as db;

#[cfg(feature = "server")]
use super::{internal_rpc_error, AuthUser, PoolExt};

/// Alignment payload for one book: link state + staleness, both lanes'
/// raw material, and both current positions.
#[post("/api/rpc/cross-format/alignment", pool: PoolExt, user: AuthUser)]
pub async fn rpc_get_alignment(uuid: String) -> Result<AlignmentView> {
    match db::cross_format::alignment_view(&pool.0, user.id, &uuid).await {
        Ok(view) => Ok(view),
        Err(db::cross_format::CrossFormatError::BookNotFound) => {
            Err(ServerFnError::new("book not found").into())
        }
        Err(e @ db::cross_format::CrossFormatError::AudioSetMismatch) => {
            Err(ServerFnError::new(e.to_string()).into())
        }
        Err(db::cross_format::CrossFormatError::Sqlx(e)) => {
            Err(internal_rpc_error("get alignment", e).into())
        }
    }
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
        match db::cross_format::set_audio_order(&pool.0, &update.book_uuid, order).await {
            Ok(()) => {}
            Err(db::cross_format::CrossFormatError::BookNotFound) => {
                return Err(ServerFnError::new("book not found").into());
            }
            Err(e @ db::cross_format::CrossFormatError::AudioSetMismatch) => {
                return Err(ServerFnError::new(format!("{e} — reopen and retry")).into());
            }
            Err(db::cross_format::CrossFormatError::Sqlx(e)) => {
                return Err(internal_rpc_error("set audio order", e).into());
            }
        }
    }
    match db::cross_format::upsert_link(
        &pool.0,
        user.id,
        &update.book_uuid,
        update.mode,
        update.primary_book_file_id,
    )
    .await
    {
        Ok(_) => Ok(()),
        Err(db::cross_format::CrossFormatError::BookNotFound) => {
            Err(ServerFnError::new("book not found").into())
        }
        Err(e @ db::cross_format::CrossFormatError::AudioSetMismatch) => {
            Err(ServerFnError::new(e.to_string()).into())
        }
        Err(db::cross_format::CrossFormatError::Sqlx(e)) => {
            Err(internal_rpc_error("confirm cross-format link", e).into())
        }
    }
}

/// Turn sync off for one book; returns whether a link existed.
#[post("/api/rpc/cross-format/unlink", pool: PoolExt, user: AuthUser)]
pub async fn rpc_unlink_cross_format(uuid: String) -> Result<bool> {
    match db::cross_format::delete_link(&pool.0, user.id, &uuid).await {
        Ok(existed) => Ok(existed),
        Err(db::cross_format::CrossFormatError::BookNotFound) => {
            Err(ServerFnError::new("book not found").into())
        }
        Err(e @ db::cross_format::CrossFormatError::AudioSetMismatch) => {
            Err(ServerFnError::new(e.to_string()).into())
        }
        Err(db::cross_format::CrossFormatError::Sqlx(e)) => {
            Err(internal_rpc_error("unlink cross-format", e).into())
        }
    }
}
