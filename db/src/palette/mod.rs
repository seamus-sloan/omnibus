//! Search palette: grouped command-palette results across books, authors,
//! series, and tags. Books go through the FTS5 MATCH path (with
//! override-aware overlays applied after hydration); taxonomy categories
//! use scoped `LIKE` substring matches against the name columns. Bounded
//! per category and scoped to `library_path`.

use sqlx::SqlitePool;

use omnibus_shared::PaletteResults;

use crate::helpers::cap_query_len;

pub mod authors;
pub mod books;
pub mod series;
pub mod tags;

#[cfg(test)]
mod tests;

pub use authors::search_authors;
pub use books::search_books;
pub use series::search_series;
pub use tags::search_tags;

/// Errors returned by the search palette.
#[derive(Debug, thiserror::Error)]
pub enum PaletteError {
    #[error(transparent)]
    Db(#[from] sqlx::Error),
}

/// Grouped command-palette results: up to 5 books, authors, series, and
/// tags scoped to `library_path`, plus server-side timing in `duration_ms`.
/// Books are matched via FTS5 (`build_fts_match`); taxonomy categories use
/// `LIKE '%q%'`. Empty/whitespace queries return `PaletteResults::default()`.
pub async fn search_palette(
    pool: &SqlitePool,
    library_path: &str,
    q: &str,
) -> Result<PaletteResults, PaletteError> {
    let trimmed = q.trim();
    if trimmed.is_empty() {
        return Ok(PaletteResults::default());
    }
    // Truncate to cap FTS5 + LIKE expression size; see `cap_query_len`.
    let trimmed: String = cap_query_len(trimmed);
    let trimmed = trimmed.as_str();

    let start = std::time::Instant::now();
    const LIMIT: i32 = 5;

    // A. Books — FTS5 MATCH with BM25 ranking, slim projection. After
    // hydration we overlay metadata_overrides so the title and author line
    // shown in the palette match the merged values the rest of the app
    // displays (FTS already matches on the merged text — see
    // `rebuild_fts_for_book` in the override write path).
    let books = search_books(pool, library_path, trimmed, LIMIT).await?;

    // Escape the query for LIKE pattern: backslash first (it's the ESCAPE char),
    // then the LIKE wildcards percent and underscore.
    let like_q = trimmed
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    let like_pattern = format!("%{like_q}%");

    // B. Authors — substring match, scoped to library, ordered by book count.
    let authors = search_authors(pool, library_path, &like_pattern, LIMIT).await?;

    // C. Series — substring match with primary author from first book.
    let series = search_series(pool, library_path, &like_pattern, LIMIT).await?;

    // D. Tags — substring match, scoped to library.
    let tags = search_tags(pool, library_path, &like_pattern, LIMIT).await?;

    let duration_ms = start.elapsed().as_millis() as u64;

    Ok(PaletteResults {
        query: trimmed.to_string(),
        books,
        authors,
        series,
        tags,
        duration_ms,
    })
}
