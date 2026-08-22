//! Boot backfill that merges `series` rows whose stored name embeds a
//! trailing index ("Mistborn #2") into the cleaned, canonical series name,
//! repoints their `books_series_link` rows, and fills `books.series_index`
//! for newly-linked books that have none. Runs once per boot from
//! `init_db`, before `sort_keys::backfill_series_sort`.

use sqlx::{SqlitePool, Transaction};

use crate::helpers::{parse_series_index, split_embedded_series_index};
use crate::taxonomy::{delete_orphan_taxonomy, resolve_or_insert_series};

/// Errors returned by [`backfill_embedded_series_index`].
#[derive(Debug, thiserror::Error)]
pub enum SeriesNormalizeError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Merge every `series` row whose name still embeds a trailing index into
/// its cleaned, canonical row, and backfill `books.series_index` for any
/// newly-linked book that had none. Idempotent: once a series row is
/// renamed/merged its name no longer matches the embedded-index pattern, so
/// a later pass skips it — the whole scan is cheap once every library has
/// converged.
pub async fn backfill_embedded_series_index(pool: &SqlitePool) -> Result<(), SeriesNormalizeError> {
    let series: Vec<(i64, String)> = sqlx::query_as("SELECT id, name FROM series")
        .fetch_all(pool)
        .await?;
    if series.is_empty() {
        return Ok(());
    }

    let mut tx = pool.begin().await?;
    let mut merged_any = false;
    for (source_id, name) in series {
        let Some((cleaned, idx)) = split_embedded_series_index(&name) else {
            continue;
        };
        merged_any = true;
        merge_series_row(&mut tx, source_id, &cleaned, &idx).await?;
    }
    // Only sweep when something actually merged — most boots find nothing to
    // do, and the sweep scans five taxonomy tables.
    if merged_any {
        delete_orphan_taxonomy(&mut tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Repoint every book linked to the fragmented `source_id` series row onto
/// the canonical row for `cleaned_name` (minting it if this is the first
/// row to clean to that name), and fill `series_index` for any of those
/// books that carries no explicit value of its own.
async fn merge_series_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_id: i64,
    cleaned_name: &str,
    embedded_idx: &str,
) -> Result<(), sqlx::Error> {
    let target_id = resolve_or_insert_series(tx, cleaned_name).await?;

    // Capture the books linked to the fragmented row *before* the union
    // below, so the index backfill only ever touches books that came from
    // this source row — never a book already sitting on the canonical row
    // for an unrelated reason.
    let source_book_ids: Vec<i64> =
        sqlx::query_scalar("SELECT book FROM books_series_link WHERE series = ?")
            .bind(source_id)
            .fetch_all(&mut **tx)
            .await?;

    if target_id != source_id {
        // Union onto the canonical row — `INSERT OR IGNORE` so a book that
        // (unusually) already links to both the fragmented and the
        // canonical row keeps just the one link — then drop the now-stale
        // source links the union replaced.
        sqlx::query(
            "INSERT OR IGNORE INTO books_series_link (book, series)
             SELECT book, ? FROM books_series_link WHERE series = ?",
        )
        .bind(target_id)
        .bind(source_id)
        .execute(&mut **tx)
        .await?;
        sqlx::query("DELETE FROM books_series_link WHERE series = ?")
            .bind(source_id)
            .execute(&mut **tx)
            .await?;
    }

    // AC2: only fills the gap — a book with its own explicit series_index
    // is never touched. Scoped to `source_book_ids` (not "every book on
    // `target_id`") so a canonical row with pre-existing, unrelated books
    // that happen to have no index of their own is never stamped.
    //
    // Left per-book rather than batched into an `id IN (...)`: boot-only and
    // idempotent (one `backfill_embedded_series_index` pass at `init_db`).
    if let Some(idx_val) = parse_series_index(embedded_idx) {
        for book_id in source_book_ids {
            sqlx::query("UPDATE books SET series_index = ? WHERE id = ? AND series_index IS NULL")
                .bind(idx_val)
                .bind(book_id)
                .execute(&mut **tx)
                .await?;
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests;
