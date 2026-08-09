//! Title/author normalization for the cross-format auto-attach match
//! key (`books.title_norm` / `books.author_norm`). Computed at index
//! time by the sync writers and backfilled once at boot by
//! [`backfill_norm_columns`]; consumed by `sync::attach`.

use sqlx::SqlitePool;

/// Errors returned by [`backfill_norm_columns`].
#[derive(Debug, thiserror::Error)]
pub enum NormalizeError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Normalize a title into its match key: diacritics folded, lowercased,
/// `&` expanded to `and`, punctuation collapsed to single spaces. `None`
/// when nothing survives.
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

/// Fold `s` to `[a-z0-9 ]`, expanding `&` and collapsing everything else
/// to single spaces.
///
/// `&` is expanded rather than dropped so the two spellings of the same
/// conjunction converge on one key — without it `"Mirth & Magic"` keys as
/// `mirth magic` and a provider's `"Mirth and Magic"` as
/// `mirth and magic`, and neither the Check-In norm rung nor cross-format
/// auto-attach can bridge a difference that sits mid-string. It expands in
/// every position, glued neighbours included (`R&B` → `r and b`), because
/// `&` is only ever the word "and".
///
/// `+` deliberately gets no such treatment: it is far more often a literal
/// part of a token (`C++`, `Disney+`, `A+`) than a conjunction, so
/// expanding it would key `"C++ Primer"` as `c and and primer`.
fn normalize(s: &str) -> Option<String> {
    let folded = deunicode::deunicode(s).to_lowercase();
    let mut out = String::with_capacity(folded.len());
    let mut pending_space = false;
    for c in folded.chars() {
        if c == '&' {
            if !out.is_empty() {
                out.push(' ');
            }
            out.push_str("and");
            pending_space = true;
            continue;
        }
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
/// boot from `init_db`. Normalizes **scanned** metadata only; the
/// override-derived keys have their own boot pass
/// (`metadata_overrides::backfill_override_norm_columns`).
///
/// The `IS NULL` guard is also how a *changed* folder re-derives stored
/// keys: a migration nulls the rows whose key the change invalidated and
/// this pass recomputes them (migration 0070 did that for `&`).
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

    // `title_norm` lands as '' (not NULL) when nothing survives normalization,
    // so the `IS NULL` guard above stays idempotent. Empty strings never
    // match: the attach lookup only binds non-empty keys. `author_norm` stays
    // genuinely nullable, and a `None` here means *not derivable* — see the
    // COALESCE below.
    let updates: Vec<(i64, String, Option<String>)> = rows
        .into_iter()
        .map(|(id, title, author)| {
            (
                id,
                normalize_title(&title).unwrap_or_default(),
                author.as_deref().and_then(normalize_author),
            )
        })
        .collect();

    let mut tx = pool.begin().await?;
    for chunk in updates.chunks(NORM_UPDATE_CHUNK) {
        let values = std::iter::repeat_n("(?, ?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        // COALESCE, not a bare write: a NULL recomputed `author_norm` means
        // the key could not be *derived* — no position-0 link (the first
        // creator is on the `ignored_authors` blocklist, which leaves a
        // positional gap the sync writer doesn't) or a name that normalizes
        // to nothing. Overwriting a real stored key with that turns "I can't
        // tell" into "there is none", which is exactly what a migration that
        // hands this pass a `&`-titled row must not cause. Rows that legitimately
        // have no author already hold NULL, so preserving is a no-op for them.
        let sql = format!(
            "UPDATE books
                SET title_norm  = v.column2,
                    author_norm = COALESCE(v.column3, books.author_norm)
               FROM (VALUES {values}) AS v
              WHERE books.id = v.column1"
        );
        let mut q = sqlx::query(&sql);
        for (id, title_norm, author_norm) in chunk {
            q = q.bind(id).bind(title_norm).bind(author_norm);
        }
        q.execute(&mut *tx).await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Rows per chunk for the norm-column backfill UPDATE. Three binds per row
/// keeps a chunk under SQLite's 999-parameter cap.
const NORM_UPDATE_CHUNK: usize = 300;

#[cfg(test)]
mod tests;
