//! Search palette: grouped command-palette results across books, authors,
//! series, and tags. Books go through the FTS5 MATCH path with override-aware
//! overlays applied after hydration; taxonomy categories use scoped `LIKE`
//! substring matches against the name columns. Bounded per category, and scoped
//! to the books `helpers::visible_book_sql` admits.

use omnibus_shared::PaletteResults;
use sqlx::SqlitePool;

use crate::helpers::cap_query_len;

pub mod authors;
pub mod books;
pub mod series;
pub mod tags;

#[cfg(test)]
mod tests;

// Plain `use` (not `pub use`) — `db/src/lib.rs` does `pub use palette::*`,
// so a `pub use authors::search_authors` here would promote the per-arm
// helpers into the `omnibus_db::*` prelude. Only `search_palette` was
// publicly exposed pre-split; the arm helpers stay reachable internally
// for `search_palette` and externally only via the fully-qualified
// `palette::authors::search_authors` path.
use authors::{count_authors_for_paths, search_authors_for_paths};
use books::search_books_for_paths;
use series::{count_series_for_paths, search_series_for_paths};
use tags::{count_tags_for_paths, search_tags_for_paths};

#[cfg(test)]
use authors::{count_authors, search_authors};
#[cfg(test)]
use series::{count_series, search_series};
#[cfg(test)]
use tags::{count_tags, search_tags};

/// Errors returned by the search palette.
#[derive(Debug, thiserror::Error)]
pub enum PaletteError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

impl From<crate::metadata_overrides::MetadataOverridesError> for PaletteError {
    fn from(e: crate::metadata_overrides::MetadataOverridesError) -> Self {
        match e {
            crate::metadata_overrides::MetadataOverridesError::Db(inner) => PaletteError::Db(inner),
            crate::metadata_overrides::MetadataOverridesError::Serialization(inner) => {
                PaletteError::Db(sqlx::Error::Decode(Box::new(inner)))
            }
            crate::metadata_overrides::MetadataOverridesError::Io(inner) => {
                PaletteError::Db(sqlx::Error::Io(inner))
            }
            // Bulk-write-only variants that can't arise on the palette read
            // path; folded with their message preserved rather than panicking
            // (mirrors `BooksError`'s `From<MetadataOverridesError>`).
            other @ (crate::metadata_overrides::MetadataOverridesError::BookNotFound(_)
            | crate::metadata_overrides::MetadataOverridesError::TooManyValues {
                ..
            }) => PaletteError::Db(sqlx::Error::Protocol(other.to_string())),
        }
    }
}

/// Per-category display cap. Each arm returns at most this many hits; the
/// uncapped per-category totals (`book_total` etc.) are computed separately.
const LIMIT: i32 = 5;

/// Grouped command-palette results for one library path.
///
/// Returns up to 5 books, authors, series, and tags plus server-side timing in
/// `duration_ms`.
/// Books are matched via FTS5 (`build_fts_match`); taxonomy categories use
/// `LIKE '%q%'`. Empty/whitespace queries return `PaletteResults::default()`.
pub async fn search_palette(
    pool: &SqlitePool,
    library_path: &str,
    q: &str,
) -> Result<PaletteResults, PaletteError> {
    search_palette_for_paths(pool, &[library_path], q).await
}

/// Search every configured library path as one ranked palette result set.
pub async fn search_palette_for_paths(
    pool: &SqlitePool,
    library_paths: &[&str],
    q: &str,
) -> Result<PaletteResults, PaletteError> {
    if library_paths.is_empty() {
        return Ok(PaletteResults::default());
    }
    let trimmed = q.trim();
    if trimmed.is_empty() {
        return Ok(PaletteResults::default());
    }
    // Truncate to cap FTS5 + LIKE expression size; see `cap_query_len`.
    let trimmed: String = cap_query_len(trimmed);
    let trimmed = trimmed.as_str();

    let start = std::time::Instant::now();

    // A. Books — FTS5 MATCH with BM25 ranking, slim projection. After
    // hydration we overlay metadata_overrides so the title and author line
    // shown in the palette match the merged values the rest of the app
    // displays (FTS already matches on the merged text — see
    // `rebuild_fts_for_book` in the override write path). The books arm also
    // returns its uncapped total in the same FTS5 pass.
    let (books, book_total) = search_books_for_paths(pool, library_paths, trimmed, LIMIT).await?;

    // Escape the query for LIKE pattern: backslash first (it's the ESCAPE char),
    // then the LIKE wildcards percent and underscore.
    let like_q = trimmed
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let like_pattern = format!("%{like_q}%");

    // B. Authors — substring match, scoped to library, ordered by book count.
    let authors = search_authors_for_paths(pool, library_paths, &like_pattern, LIMIT).await?;

    // C. Series — substring match with primary author from first book.
    let series = search_series_for_paths(pool, library_paths, &like_pattern, LIMIT).await?;

    // D. Tags — substring match, scoped to library.
    let tags = search_tags_for_paths(pool, library_paths, &like_pattern, LIMIT).await?;

    // Uncapped per-category totals for the full-page results header. Each is a
    // cheap COUNT over the same visibility predicate the arm uses; books got
    // theirs in-pass above. Skipped when the capped vec already holds the whole
    // match set (len < LIMIT ⇒ no more rows to count).
    let author_total = total_for(authors.len(), || {
        count_authors_for_paths(pool, library_paths, &like_pattern)
    })
    .await?;
    let series_total = total_for(series.len(), || {
        count_series_for_paths(pool, library_paths, &like_pattern)
    })
    .await?;
    let tag_total = total_for(tags.len(), || {
        count_tags_for_paths(pool, library_paths, &like_pattern)
    })
    .await?;

    let duration_ms = u64::try_from(start.elapsed().as_millis()).unwrap_or(u64::MAX);

    Ok(PaletteResults {
        query: trimmed.to_string(),
        books,
        authors,
        series,
        tags,
        duration_ms,
        book_total: u32::try_from(book_total).unwrap_or(u32::MAX),
        author_total,
        series_total,
        tag_total,
    })
}

/// Resolve a category's uncapped total, skipping the COUNT query when the
/// capped result already holds the entire match set. The arms cap at `LIMIT`,
/// so a vec shorter than the cap is exhaustive and `len` *is* the total — only
/// a full vec might be hiding further matches worth counting.
async fn total_for<F, Fut>(returned: usize, count: F) -> Result<u32, PaletteError>
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = Result<i64, PaletteError>>,
{
    if returned < LIMIT as usize {
        return Ok(u32::try_from(returned).unwrap_or(u32::MAX));
    }
    Ok(u32::try_from(count().await?).unwrap_or(u32::MAX))
}
