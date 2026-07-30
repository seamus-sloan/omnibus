//! One-shot repair pass for Kobo-synced annotations that predate CFI
//! derivation: rows holding only a `kobo_location` anchor get their
//! `epub_cfi_range` derived so the web/iOS readers can render them. Spawned
//! in the background at server boot; a row that stays underivable (no kepub
//! cache, diverged text) is retried on the next boot, which is cheap — the
//! candidate query returns nothing once everything derivable is done.

use std::collections::HashMap;

use sqlx::SqlitePool;

use crate::kobo_position::{annotation_cfis, parse_annotation_location, KoboSpanRange};

/// What one backfill pass accomplished, for the boot log.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct BackfillStats {
    /// Rows that now carry a derived CFI.
    pub derived: usize,
    /// Candidate rows left without one (unparseable anchor, missing kepub
    /// cache or source file, or a derivation miss).
    pub unresolved: usize,
}

/// Derive `epub_cfi_range` for every annotation row that has a
/// `kobo_location` but no CFI yet, one book at a time. Deliberately does
/// **not** touch `updated_at`: the derivation changes nothing a Kobo device
/// consumes, so it must not re-report every book through `checkforchanges`.
pub async fn backfill_kobo_annotation_cfis(pool: &SqlitePool) -> anyhow::Result<BackfillStats> {
    let rows = sqlx::query_as::<_, (i64, String, String)>(
        "SELECT id, book_uuid, kobo_location FROM annotations
         WHERE kobo_location IS NOT NULL AND epub_cfi_range IS NULL",
    )
    .fetch_all(pool)
    .await?;

    let mut by_book: HashMap<String, Vec<(i64, String)>> = HashMap::new();
    for (id, book_uuid, kobo_location) in rows {
        by_book
            .entry(book_uuid)
            .or_default()
            .push((id, kobo_location));
    }

    let mut stats = BackfillStats::default();
    for (book_uuid, rows) in by_book {
        match backfill_book(pool, &book_uuid, &rows).await {
            Ok(derived) => {
                stats.derived += derived;
                stats.unresolved += rows.len() - derived;
            }
            Err(e) => {
                tracing::warn!(book_uuid, error = %e, "annotation CFI backfill failed for book");
                stats.unresolved += rows.len();
            }
        }
    }
    Ok(stats)
}

/// Backfill one book's rows: resolve its kepub + source paths, derive all
/// anchors in one batch, write back the hits. Returns how many derived.
async fn backfill_book(
    pool: &SqlitePool,
    book_uuid: &str,
    rows: &[(i64, String)],
) -> anyhow::Result<usize> {
    let Some(book_id) = crate::resolve_book_id_by_uuid(pool, book_uuid).await? else {
        return Ok(0);
    };
    let kepub = crate::kepub_path(book_id);
    if !tokio::fs::try_exists(&kepub).await.unwrap_or(false) {
        return Ok(0);
    }
    let Some(source) = crate::book_file_path(pool, book_id, "EPUB").await? else {
        return Ok(0);
    };

    // Unparseable anchors keep their row slot so results zip back to ids;
    // they simply derive to nothing.
    let ranges: Vec<Option<KoboSpanRange>> = rows
        .iter()
        .map(|(_, loc)| parse_annotation_location(loc))
        .collect();
    let parseable: Vec<KoboSpanRange> = ranges.iter().flatten().cloned().collect();
    if parseable.is_empty() {
        return Ok(0);
    }
    let cfis =
        tokio::task::spawn_blocking(move || annotation_cfis(&kepub, &source, &parseable)).await??;

    let mut derived = 0;
    let mut cfis = cfis.into_iter();
    for ((id, _), range) in rows.iter().zip(&ranges) {
        let Some(cfi) = range.as_ref().and_then(|_| cfis.next().flatten()) else {
            continue;
        };
        sqlx::query("UPDATE annotations SET epub_cfi_range = ? WHERE id = ?")
            .bind(&cfi)
            .bind(id)
            .execute(pool)
            .await?;
        derived += 1;
    }
    Ok(derived)
}

#[cfg(test)]
mod tests;
