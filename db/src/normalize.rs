//! Title/author normalization for the cross-format auto-attach match
//! key (`books.title_norm` / `books.author_norm`). Computed at index
//! time by the sync writers and backfilled once at boot by
//! [`backfill_norm_columns`]; consumed by `sync::attach`.

use sqlx::SqlitePool;

/// Errors returned by the title/author normalization backfill. Wraps
/// `sqlx::Error` so it doesn't leak across the module boundary per
/// rule 02 (Error handling) § Boundary.
#[derive(Debug, thiserror::Error)]
pub enum NormalizeError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Normalize a title into its match key: diacritics folded, lowercased,
/// punctuation collapsed to single spaces. `None` when nothing survives.
///
/// Deliberately exact — no article stripping, no substring or fuzzy
/// matching — so *Dune* never matches *Dune Messiah*. A missed match
/// costs one manual merge; a false positive silently corrupts a book.
pub fn normalize_title(s: &str) -> Option<String> {
    normalize(s)
}

/// Normalize an author name into its match key. Same folding as
/// [`normalize_title`], plus a single-comma `"Last, First"` is swapped
/// to `"First Last"` first — EPUB `file_as` values and audio artist
/// tags disagree on name order.
pub fn normalize_author(s: &str) -> Option<String> {
    let parts: Vec<&str> = s.split(',').collect();
    if parts.len() == 2 {
        return normalize(&format!("{} {}", parts[1], parts[0]));
    }
    normalize(s)
}

fn normalize(s: &str) -> Option<String> {
    let folded = deunicode::deunicode(s).to_lowercase();
    let mut out = String::with_capacity(folded.len());
    let mut pending_space = false;
    for c in folded.chars() {
        if c.is_ascii_alphanumeric() {
            if pending_space && !out.is_empty() {
                out.push(' ');
            }
            pending_space = false;
            out.push(c);
        } else {
            pending_space = true;
        }
    }
    Some(out).filter(|s| !s.is_empty())
}

/// One-time backfill of `books.(title_norm, author_norm)` for rows
/// indexed before migration 0016. Idempotent — only touches rows where
/// `title_norm IS NULL` — and cheap once caught up, so it runs on every
/// boot from `init_db`. Normalizes **scanned** metadata only
/// (`metadata_overrides` is a read layer and deliberately ignored).
pub async fn backfill_norm_columns(pool: &SqlitePool) -> Result<(), NormalizeError> {
    let rows: Vec<(i64, String, Option<String>)> = sqlx::query_as(
        "SELECT b.id, b.title, a.name
           FROM books b
           LEFT JOIN books_authors_link l ON l.book = b.id AND l.position = 0
           LEFT JOIN authors a ON a.id = l.author
          WHERE b.title_norm IS NULL",
    )
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Ok(());
    }

    let mut tx = pool.begin().await?;
    for (id, title, author) in rows {
        // `title_norm` lands as '' (not NULL) when nothing survives
        // normalization, so the IS NULL guard above stays idempotent.
        // Empty strings never match: the attach lookup only binds
        // non-empty keys.
        sqlx::query("UPDATE books SET title_norm = ?, author_norm = ? WHERE id = ?")
            .bind(normalize_title(&title).unwrap_or_default())
            .bind(author.as_deref().and_then(normalize_author))
            .bind(id)
            .execute(&mut *tx)
            .await?;
    }
    tx.commit().await?;
    Ok(())
}

#[cfg(test)]
mod tests;
