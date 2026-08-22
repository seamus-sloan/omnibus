//! Audio file ordering: the validated two-phase `book_files.ordinal`
//! write behind the alignment modal's re-order, shared with the confirm
//! path so a re-order can never commit without its link.

use sqlx::SqlitePool;

use crate::{resolve_book_id_by_uuid, resolve_canonical_book_uuid};

use super::mapping::audio_files;
use super::CrossFormatError;

/// Persist a re-ordering of the book's audio files. `order` must name
/// exactly the book's current audio file set — anything else refuses with
/// [`CrossFormatError::AudioSetMismatch`], so a stale modal can't scramble
/// ordinals after a re-index. Two-phase ordinal write because
/// `UNIQUE(book_id, format, ordinal)` is enforced per row.
pub async fn set_audio_order(
    pool: &SqlitePool,
    book_uuid: &str,
    order: &[i64],
) -> Result<(), CrossFormatError> {
    let book_uuid = resolve_canonical_book_uuid(pool, book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;
    let book_id = resolve_book_id_by_uuid(pool, &book_uuid)
        .await?
        .ok_or(CrossFormatError::BookNotFound)?;
    let mut tx = pool.begin().await?;
    apply_audio_order_tx(&mut tx, book_id, order).await?;
    tx.commit().await?;
    Ok(())
}

/// The validated two-phase ordinal write, inside the caller's transaction —
/// [`super::confirm_link`] runs it in the same transaction as the link
/// upsert so a confirm can never commit a re-order without its link (or
/// vice versa).
pub(super) async fn apply_audio_order_tx(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    order: &[i64],
) -> Result<(), CrossFormatError> {
    let current: std::collections::HashSet<i64> = audio_files(&mut **tx, book_id)
        .await?
        .into_iter()
        .map(|f| f.book_file_id)
        .collect();
    let proposed: std::collections::HashSet<i64> = order.iter().copied().collect();
    if proposed.len() != order.len() || proposed != current {
        return Err(CrossFormatError::AudioSetMismatch);
    }
    for (i, id) in order.iter().enumerate() {
        sqlx::query("UPDATE book_files SET ordinal = ? WHERE id = ? AND book_id = ?")
            .bind(-(i as i64) - 1)
            .bind(id)
            .bind(book_id)
            .execute(&mut **tx)
            .await?;
    }
    sqlx::query(
        "UPDATE book_files SET ordinal = -ordinal - 1
         WHERE book_id = ? AND format IN ('M4B', 'M4A', 'MP3') AND ordinal < 0",
    )
    .bind(book_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
