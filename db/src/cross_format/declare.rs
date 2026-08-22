//! "Synced here" declarations: pair the declaring surface's position with
//! the counterpart format's stored row and record it as a user anchor.
//! User anchors outrank chapter anchors everywhere the mapping runs.

use omnibus_shared::cross_format::CrossFormatLinkMode;
use omnibus_shared::ProgressFormat;
use sqlx::SqlitePool;

use crate::{progress, resolve_book_id_by_uuid, resolve_canonical_book_uuid};

use super::link::get_link;
use super::mapping::{
    audio_files, audio_fraction, audio_timeline, epub_source_fraction, fraction_for_cfi,
    link_is_stale, snapshot_json, AudioTimeline,
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

    // Resolved in memory first, so every refusal below precedes any write.
    let (link, created_here) = match get_link(pool, user_id, &book_uuid).await? {
        Some(link) => (link, false),
        None => (auto_sequence_link(pool, book_id).await?, true),
    };
    let (text_frac, audio_global) =
        declared_pair(pool, user_id, book_id, &book_uuid, &link, decl).await?;

    // One transaction, mirroring `confirm_link`: a link left behind without
    // its anchor carries `follow = 1` and syncs positions the caller was
    // told the declaration never recorded.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if created_here {
        insert_auto_link(&mut tx, user_id, &book_uuid, &link.audio_snapshot).await?;
    }
    store_anchor(&mut tx, user_id, &book_uuid, &link, text_frac, audio_global).await?;
    tx.commit().await?;

    get_link(pool, user_id, &book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)
}

/// The link a single-audio-file book is auto-confirmed with, built in
/// memory so the declaration resolves against it before anything is
/// written. A multi-file book refuses with [`CrossFormatError::LinkRequired`].
async fn auto_sequence_link(
    pool: &SqlitePool,
    book_id: i64,
) -> Result<CrossFormatLink, CrossFormatError> {
    let files = audio_files(pool, book_id).await?;
    if files.len() != 1 {
        return Err(CrossFormatError::LinkRequired);
    }
    Ok(CrossFormatLink {
        mode: CrossFormatLinkMode::Sequence,
        primary_book_file_id: None,
        audio_snapshot: snapshot_json(&files),
        // Placeholder — the INSERT stamps the value the caller reads back.
        confirmed_at: 0,
        follow: true,
        user_anchors: Vec::new(),
    })
}

/// Write the auto-confirmed sequence link inside the declaration's
/// transaction. Plain INSERT on purpose: a racing confirm trips the unique
/// constraint and rolls the declaration back, rather than filing this
/// anchor against an alignment it was never measured on.
async fn insert_auto_link(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    user_id: i64,
    book_uuid: &str,
    audio_snapshot: &str,
) -> Result<(), CrossFormatError> {
    sqlx::query(
        "INSERT INTO cross_format_links
            (user_id, book_uuid, mode, primary_book_file_id, audio_snapshot, confirmed_at, follow)
         VALUES (?, ?, 'sequence', NULL, ?, CAST(strftime('%s','now') AS INTEGER), 1)",
    )
    .bind(user_id)
    .bind(book_uuid)
    .bind(audio_snapshot)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Resolve the declared pair — `(text fraction, audio global seconds)` —
/// against the link the anchor will be stored on. Read-only by design:
/// [`declare_sync_point`] runs it before opening its write transaction.
async fn declared_pair(
    pool: &SqlitePool,
    user_id: i64,
    book_id: i64,
    book_uuid: &str,
    link: &CrossFormatLink,
    decl: &omnibus_shared::cross_format::DeclareSyncPoint,
) -> Result<(f64, f64), CrossFormatError> {
    if link_is_stale(pool, book_id, link).await? {
        return Err(CrossFormatError::AudioSetMismatch);
    }
    let timeline = audio_timeline(pool, book_id, link)
        .await?
        .ok_or(CrossFormatError::LinkRequired)?;

    match decl.format {
        ProgressFormat::Epub => {
            ebook_declared_pair(pool, user_id, book_id, book_uuid, &timeline, decl).await
        }
        ProgressFormat::Audio => {
            audio_declared_pair(pool, user_id, book_id, book_uuid, &timeline, decl).await
        }
    }
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
/// re-declaration of the same spot) and turn follow mode on, inside the
/// caller's transaction.
async fn store_anchor(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    user_id: i64,
    book_uuid: &str,
    link: &CrossFormatLink,
    text_frac: f64,
    audio_global: f64,
) -> Result<(), CrossFormatError> {
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
    .execute(&mut **tx)
    .await?;
    Ok(())
}
