//! The per-user link row: the declaration that one book's two formats
//! describe the same text and may sync. Confirming a link is what turns
//! sync on; every read degrades to "no link" rather than a wrong answer.

use omnibus_shared::cross_format::CrossFormatLinkMode;
use sqlx::SqlitePool;

use crate::{resolve_book_id_by_uuid, resolve_canonical_book_uuid};

use super::mapping::{audio_files, snapshot_json};
use super::order::apply_audio_order_tx;
use super::CrossFormatError;

/// One confirmed link row — the user's declaration that this book's
/// formats describe the same text and may sync.
#[derive(Debug, Clone, PartialEq)]
pub struct CrossFormatLink {
    pub mode: CrossFormatLinkMode,
    pub primary_book_file_id: Option<i64>,
    pub audio_snapshot: String,
    pub confirmed_at: i64,
    /// Follow mode: resolve-at-read instead of offer-based prompts.
    pub follow: bool,
    /// "Synced here" declarations as `(ebook_fraction, audio_global
    /// seconds)` pairs, sorted by fraction; they outrank chapter anchors.
    pub user_anchors: Vec<(f64, f64)>,
}

fn parse_user_anchors(raw: &str) -> Vec<(f64, f64)> {
    // A row this module didn't write (or a corrupt one) degrades to no
    // user anchors rather than an error — mapping still works.
    serde_json::from_str::<Vec<(f64, f64)>>(raw).unwrap_or_default()
}

fn mode_str(mode: CrossFormatLinkMode) -> &'static str {
    match mode {
        CrossFormatLinkMode::Sequence => "sequence",
        CrossFormatLinkMode::Narrations => "narrations",
    }
}

fn parse_mode(raw: &str) -> CrossFormatLinkMode {
    // The CHECK constraint rules out anything else; an unknown string
    // defaults to narrations, the maximally conservative reading — with
    // no primary recorded it refuses to map at all, rather than
    // concatenating files the user never declared sequential.
    match raw {
        "sequence" => CrossFormatLinkMode::Sequence,
        _ => CrossFormatLinkMode::Narrations,
    }
}

/// Confirm (or re-confirm) a link. Snapshots the book's current audio file
/// set so later file changes read as stale, and replaces any existing row
/// wholesale — a re-confirmation is a fresh declaration, not a merge.
pub async fn upsert_link(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    mode: CrossFormatLinkMode,
    primary_book_file_id: Option<i64>,
) -> Result<CrossFormatLink, CrossFormatError> {
    confirm_link(pool, user_id, book_uuid, mode, primary_book_file_id, None).await
}

/// Confirm a link, atomically applying an audio re-order when one rides
/// along. One transaction on purpose: the re-order is a library-wide
/// `book_files.ordinal` mutation, and committing it without the link (or
/// the link against a half-applied order) leaves state no confirm ever
/// described — the snapshot must also be taken from the ordinals the same
/// transaction just wrote.
pub async fn confirm_link(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    mode: CrossFormatLinkMode,
    primary_book_file_id: Option<i64>,
    audio_order: Option<&[i64]>,
) -> Result<CrossFormatLink, CrossFormatError> {
    let book_uuid = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;
    let book_id = resolve_book_id_by_uuid(pool, &book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;
    // BEGIN IMMEDIATE mirrors `upsert_progress` — take the write lock up
    // front so a concurrent writer can't stale this snapshot mid-confirm.
    let mut tx = pool.begin_with("BEGIN IMMEDIATE").await?;
    if let Some(order) = audio_order {
        apply_audio_order_tx(&mut tx, book_id, order).await?;
    }
    let snapshot = snapshot_json(&audio_files(&mut *tx, book_id).await?);
    // Confirming the alignment IS turning sync on — the modal's confirm
    // button says exactly that, and a linked book that still prompts on
    // every surface switch reads as broken (live QA on #1944). The follow
    // toggle can still switch it off afterward; user anchors survive only
    // when the audio set (ordinal order included), mode, AND narrations
    // primary are all unchanged — anchors store global seconds on one
    // concrete timeline, and any of those three changing makes that
    // timeline a different ruler the stored pairs no longer measure.
    sqlx::query(
        "INSERT INTO cross_format_links
            (user_id, book_uuid, mode, primary_book_file_id, audio_snapshot, confirmed_at, follow)
         VALUES (?, ?, ?, ?, ?, CAST(strftime('%s','now') AS INTEGER), 1)
         ON CONFLICT(user_id, book_uuid) DO UPDATE SET
            mode = excluded.mode,
            follow = 1,
            primary_book_file_id = excluded.primary_book_file_id,
            user_anchors = CASE
                WHEN cross_format_links.audio_snapshot = excluded.audio_snapshot
                 AND cross_format_links.mode = excluded.mode
                 AND COALESCE(cross_format_links.primary_book_file_id, -1)
                     = COALESCE(excluded.primary_book_file_id, -1)
                THEN cross_format_links.user_anchors ELSE '[]' END,
            audio_snapshot = excluded.audio_snapshot,
            confirmed_at = excluded.confirmed_at",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .bind(mode_str(mode))
    .bind(primary_book_file_id)
    .bind(&snapshot)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    get_link(pool, user_id, &book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)
}

/// The stored link for `(user, book)`, if the user has confirmed one.
pub async fn get_link(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<Option<CrossFormatLink>, CrossFormatError> {
    let Some(book_uuid) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(None);
    };
    let row: Option<(String, Option<i64>, String, i64, i64, String)> = sqlx::query_as(
        "SELECT mode, primary_book_file_id, audio_snapshot, confirmed_at, follow, user_anchors
         FROM cross_format_links WHERE user_id = ? AND book_uuid = ?",
    )
    .bind(user_id)
    .bind(&book_uuid)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(
        |(mode, primary_book_file_id, audio_snapshot, confirmed_at, follow, user_anchors)| {
            CrossFormatLink {
                mode: parse_mode(&mode),
                primary_book_file_id,
                audio_snapshot,
                confirmed_at,
                follow: follow != 0,
                user_anchors: parse_user_anchors(&user_anchors),
            }
        },
    ))
}

/// Flip follow mode on an existing link. Returns whether a link existed.
pub async fn set_follow(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    enabled: bool,
) -> Result<bool, CrossFormatError> {
    // Unknown book is 404 territory; `Ok(false)` is reserved for a known
    // book with no link to flip.
    let book_uuid = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;
    let result =
        sqlx::query("UPDATE cross_format_links SET follow = ? WHERE user_id = ? AND book_uuid = ?")
            .bind(enabled as i64)
            .bind(user_id)
            .bind(&book_uuid)
            .execute(pool)
            .await?;
    Ok(result.rows_affected() > 0)
}

/// Remove a link (sync off). Returns whether a row existed.
pub async fn delete_link(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
) -> Result<bool, CrossFormatError> {
    let Some(book_uuid) = resolve_canonical_book_uuid(pool, book_uuid).await? else {
        return Ok(false);
    };
    let result = sqlx::query("DELETE FROM cross_format_links WHERE user_id = ? AND book_uuid = ?")
        .bind(user_id)
        .bind(&book_uuid)
        .execute(pool)
        .await?;
    Ok(result.rows_affected() > 0)
}
