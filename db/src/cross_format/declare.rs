//! "Synced here" declarations: pair the declaring surface's position with
//! the counterpart format's stored row and record it as a user anchor.
//! User anchors outrank chapter anchors everywhere the mapping runs.

use omnibus_shared::cross_format::CrossFormatLinkMode;
use omnibus_shared::ProgressFormat;
use sqlx::SqlitePool;

use crate::{progress, resolve_book_id_by_uuid, resolve_canonical_book_uuid};

use super::link::{delete_link, get_link, upsert_link};
use super::mapping::{
    audio_files, audio_fraction, audio_timeline, epub_source_fraction, fraction_for_cfi,
    link_is_stale, AudioTimeline,
};
use super::{CrossFormatError, CrossFormatLink};

/// Text-fraction slack inside which a new "synced here" declaration
/// replaces an existing pair instead of stacking beside it.
// Tight on purpose: on a marks-less multi-hour book every declaration is
// hard-won calibration, and 2% of a 50-hour timeline discarded re-syncs an
// hour apart. Only a true re-declaration of the same spot replaces.
const SYNC_POINT_REPLACE_SLACK: f64 = 0.005;

/// Record a "synced here" declaration: pair the declaring surface's
/// position with the counterpart format's stored row, store it as a user
/// anchor, and turn follow mode on. A single-audio-file book with no link
/// yet is auto-confirmed as a sequence link (unambiguous); a multi-file
/// book must confirm the alignment first — the declaration never guesses
/// an order.
pub async fn declare_sync_point(
    pool: &SqlitePool,
    user_id: i64,
    decl: &omnibus_shared::cross_format::DeclareSyncPoint,
) -> Result<CrossFormatLink, CrossFormatError> {
    let book_uuid = resolve_canonical_book_uuid(pool, &decl.book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;
    let book_id = resolve_book_id_by_uuid(pool, &book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;

    let (link, created_here) = match get_link(pool, user_id, &book_uuid).await? {
        Some(link) => (link, false),
        None => {
            if audio_files(pool, book_id).await?.len() != 1 {
                return Err(CrossFormatError::LinkRequired);
            }
            let link = upsert_link(
                pool,
                user_id,
                &book_uuid,
                CrossFormatLinkMode::Sequence,
                None,
            )
            .await?;
            (link, true)
        }
    };
    // A declaration that fails past this point must not leave behind the
    // link this call just auto-created — the UI reported failure, and a
    // silently-confirmed follow link would start moving positions the
    // user never agreed to sync. Best-effort compensation, not a
    // transaction: the link insert has value only alongside its anchor.
    let result = declare_against_link(pool, user_id, book_id, &book_uuid, &link, decl).await;
    if created_here && result.is_err() {
        let _ = delete_link(pool, user_id, &book_uuid).await;
    }
    result
}

/// The declaration proper, against an existing (or just-created) link —
/// split from [`declare_sync_point`] so a failure there can compensate by
/// removing a link it auto-created.
async fn declare_against_link(
    pool: &SqlitePool,
    user_id: i64,
    book_id: i64,
    book_uuid: &str,
    link: &CrossFormatLink,
    decl: &omnibus_shared::cross_format::DeclareSyncPoint,
) -> Result<CrossFormatLink, CrossFormatError> {
    if link_is_stale(pool, book_id, link).await? {
        return Err(CrossFormatError::AudioSetMismatch);
    }
    let timeline = audio_timeline(pool, book_id, link)
        .await?
        .ok_or(CrossFormatError::LinkRequired)?;

    let (text_frac, audio_global) = match decl.format {
        ProgressFormat::Epub => {
            ebook_declared_pair(pool, user_id, book_id, book_uuid, &timeline, decl).await?
        }
        ProgressFormat::Audio => {
            audio_declared_pair(pool, user_id, book_id, book_uuid, &timeline, decl).await?
        }
    };

    store_anchor(pool, user_id, book_uuid, link, text_frac, audio_global).await
}

/// A declaration made from the reader: the CFI (or client fraction) names
/// the text spot, and the stored audio row supplies its counterpart.
async fn ebook_declared_pair(
    pool: &SqlitePool,
    user_id: i64,
    book_id: i64,
    book_uuid: &str,
    timeline: &AudioTimeline,
    decl: &omnibus_shared::cross_format::DeclareSyncPoint,
) -> Result<(f64, f64), CrossFormatError> {
    // The declared CFI names the spot on the server's own ruler —
    // prefer it over the client fraction, which the web reader
    // measures on epub.js's different locations scale.
    let cfi_frac = match decl.epub_cfi.clone() {
        Some(cfi) => fraction_for_cfi(pool, book_id, cfi).await,
        None => None,
    };
    let frac = cfi_frac
        .or_else(|| decl.ebook_fraction.filter(|f| (0.0..=1.0).contains(f)))
        .ok_or(CrossFormatError::CounterpartMissing)?;
    let row = progress::get_progress(pool, user_id, book_uuid, ProgressFormat::Audio)
        .await?
        .ok_or(CrossFormatError::CounterpartMissing)?;
    let seconds = row
        .audio_position_seconds
        .ok_or(CrossFormatError::CounterpartMissing)?;
    let file_id = row
        .book_file_id
        .or_else(|| (timeline.files.len() == 1).then(|| timeline.files[0].book_file_id))
        .ok_or(CrossFormatError::CounterpartMissing)?;
    let global = audio_fraction(timeline, file_id, seconds)
        .ok_or(CrossFormatError::CounterpartMissing)?
        * timeline.total_seconds;
    Ok((frac, global))
}

/// A declaration made from the player: the declared seconds name the audio
/// spot, and the stored reading row supplies its counterpart.
async fn audio_declared_pair(
    pool: &SqlitePool,
    user_id: i64,
    book_id: i64,
    book_uuid: &str,
    timeline: &AudioTimeline,
    decl: &omnibus_shared::cross_format::DeclareSyncPoint,
) -> Result<(f64, f64), CrossFormatError> {
    let seconds = decl
        .audio_seconds
        .filter(|s| *s >= 0.0)
        .ok_or(CrossFormatError::CounterpartMissing)?;
    let file_id = decl
        .audio_book_file_id
        .or_else(|| (timeline.files.len() == 1).then(|| timeline.files[0].book_file_id))
        .ok_or(CrossFormatError::CounterpartMissing)?;
    let global = audio_fraction(timeline, file_id, seconds)
        .ok_or(CrossFormatError::CounterpartMissing)?
        * timeline.total_seconds;
    let row = progress::get_progress(pool, user_id, book_uuid, ProgressFormat::Epub)
        .await?
        .ok_or(CrossFormatError::CounterpartMissing)?;
    let frac = epub_source_fraction(pool, book_id, &row)
        .await
        .ok_or(CrossFormatError::CounterpartMissing)?;
    Ok((frac, global))
}

/// Fold the declared pair into the link's stored anchors (replacing a
/// re-declaration of the same spot) and turn follow mode on.
async fn store_anchor(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    link: &CrossFormatLink,
    text_frac: f64,
    audio_global: f64,
) -> Result<CrossFormatLink, CrossFormatError> {
    let mut anchors: Vec<(f64, f64)> = link
        .user_anchors
        .iter()
        .copied()
        .filter(|(t, _)| (t - text_frac).abs() >= SYNC_POINT_REPLACE_SLACK)
        .collect();
    anchors.push((text_frac, audio_global));
    anchors.sort_by(|a, b| a.0.total_cmp(&b.0));
    let json = serde_json::to_string(&anchors).unwrap_or_else(|_| "[]".into());

    sqlx::query(
        "UPDATE cross_format_links SET user_anchors = ?, follow = 1
         WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(&json)
    .bind(user_id)
    .bind(book_uuid)
    .execute(pool)
    .await?;
    get_link(pool, user_id, book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)
}
