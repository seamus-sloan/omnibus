//! Full-library facet-count aggregate. [`library_facets`] tallies the
//! sidebar facets (authors / series / formats / tags / genres) across the *entire*
//! configured library in one grouped query each, so the landing sidebar stays
//! correct even though the list itself is keyset-paginated and the client only
//! holds one page. Counts are deliberately over the unfiltered library —
//! mirroring the former client-side `facet_counts`, which tallied the full
//! hydrated list regardless of the active filter selection.

use omnibus_shared::{FacetCount, FacetCounts};
use sqlx::SqlitePool;

/// Row cap for each facet group, mirroring `discovery/tags.rs`'s
/// `TAG_CLOUD_LIMIT` order of magnitude — comfortably covers the distinct
/// authors/series/formats/tags of a real library while still bounding a
/// pathological one.
const FACET_LIMIT: i64 = 1_000;

/// Per-facet book counts for `library_paths`, ordered by count descending then
/// value ascending (the order the sidebar renders). Empty `library_paths`
/// returns empty facets. A book counts iff it's file-backed under a configured
/// library or has a physical copy (matching the list read paths); a fileless
/// book with no physical copy is excluded.
pub async fn library_facets(
    pool: &SqlitePool,
    library_paths: &[&str],
) -> Result<FacetCounts, super::BooksError> {
    if library_paths.is_empty() {
        return Ok(FacetCounts::default());
    }
    let ph = super::page::placeholders(library_paths.len());

    Ok(FacetCounts {
        authors: author_facets(pool, &ph, library_paths).await?,
        series: series_facets(pool, &ph, library_paths).await?,
        formats: format_facets(pool, &ph, library_paths).await?,
        tags: tag_facets(pool, &ph, library_paths).await?,
        genres: genre_facets(pool, &ph, library_paths).await?,
    })
}

/// Author facet counts: books-per-author over the visible library (file-backed
/// under a configured root, or with a physical copy).
async fn author_facets(
    pool: &SqlitePool,
    ph: &str,
    library_paths: &[&str],
) -> Result<Vec<FacetCount>, sqlx::Error> {
    facet_query(
        pool,
        &format!(
            r"
            SELECT a.name AS value, COUNT(*) AS count
              FROM books_authors_link bal
              JOIN authors a ON a.id = bal.author
              JOIN books b ON b.id = bal.book
              JOIN scan_roots l ON l.id = b.library_id
             WHERE (l.path IN ({ph})
                    AND EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id))
                OR EXISTS (SELECT 1 FROM physical_copies pc WHERE pc.book_uuid = b.uuid)
             GROUP BY a.name
             ORDER BY count DESC, value ASC
             LIMIT ?
            "
        ),
        library_paths,
    )
    .await
}

/// Series facet counts, excluding empty-name series and books with neither a
/// file nor a physical copy.
async fn series_facets(
    pool: &SqlitePool,
    ph: &str,
    library_paths: &[&str],
) -> Result<Vec<FacetCount>, sqlx::Error> {
    facet_query(
        pool,
        &format!(
            r"
            SELECT s.name AS value, COUNT(*) AS count
              FROM books_series_link bsl
              JOIN series s ON s.id = bsl.series
              JOIN books b ON b.id = bsl.book
              JOIN scan_roots l ON l.id = b.library_id
             WHERE s.name <> ''
               AND ((l.path IN ({ph})
                     AND EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id))
                 OR EXISTS (SELECT 1 FROM physical_copies pc WHERE pc.book_uuid = b.uuid))
             GROUP BY s.name
             ORDER BY count DESC, value ASC
             LIMIT ?
            "
        ),
        library_paths,
    )
    .await
}

/// Format facet counts. The JOIN to `book_files` already excludes fileless
/// books (a row with no file contributes none), so no extra EXISTS is needed.
/// `(book_id, format)` is unique, so COUNT(DISTINCT b.id) per lowercased format
/// equals the number of books carrying it.
async fn format_facets(
    pool: &SqlitePool,
    ph: &str,
    library_paths: &[&str],
) -> Result<Vec<FacetCount>, sqlx::Error> {
    facet_query(
        pool,
        &format!(
            r"
            SELECT LOWER(bf.format) AS value, COUNT(DISTINCT b.id) AS count
              FROM book_files bf
              JOIN books b ON b.id = bf.book_id
              JOIN scan_roots l ON l.id = b.library_id
             WHERE l.path IN ({ph})
             GROUP BY LOWER(bf.format)
             ORDER BY count DESC, value ASC
             LIMIT ?
            "
        ),
        library_paths,
    )
    .await
}

/// Tag facet counts, excluding the empty-name tag and fileless books.
async fn tag_facets(
    pool: &SqlitePool,
    ph: &str,
    library_paths: &[&str],
) -> Result<Vec<FacetCount>, sqlx::Error> {
    facet_query(
        pool,
        &format!(
            r"
            SELECT t.name AS value, COUNT(*) AS count
              FROM books_tags_link btl
              JOIN tags t ON t.id = btl.tag
              JOIN books b ON b.id = btl.book
              JOIN scan_roots l ON l.id = b.library_id
             WHERE t.name <> ''
               AND ((l.path IN ({ph})
                     AND EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id))
                 OR EXISTS (SELECT 1 FROM physical_copies pc WHERE pc.book_uuid = b.uuid))
             GROUP BY t.name
             ORDER BY count DESC, value ASC
             LIMIT ?
            "
        ),
        library_paths,
    )
    .await
}

/// Genre facet counts. Genres live only in `metadata_overrides` JSON
/// (migration `0066`), so this reads `json_each` over `$.genres` rather than
/// joining a link table.
///
/// The join to `genres` is what makes the facet value canonical: that table
/// is `NOCASE`-unique, so a book overridden with "sci-fi" and another with
/// "Sci-Fi" report one facet rather than two that each filter to half the
/// shelf. `get_genre_cloud` folds the same way, and the two must agree —
/// the cloud is what the filter chips are picked from. `DISTINCT b.id`
/// guards a duplicate entry inside one book's array; the visible-library
/// predicate matches the other facets.
async fn genre_facets(
    pool: &SqlitePool,
    ph: &str,
    library_paths: &[&str],
) -> Result<Vec<FacetCount>, sqlx::Error> {
    facet_query(
        pool,
        &format!(
            r"
            SELECT g.name AS value, COUNT(DISTINCT b.id) AS count
              FROM books b
              JOIN scan_roots l ON l.id = b.library_id
              JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
              JOIN json_each(mo.overrides, '$.genres') je
              JOIN genres g ON g.name = je.value COLLATE NOCASE
             WHERE json_type(mo.overrides, '$.genres') IS NOT NULL
               AND g.name <> ''
               AND ((l.path IN ({ph})
                     AND EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id))
                 OR EXISTS (SELECT 1 FROM physical_copies pc WHERE pc.book_uuid = b.uuid))
             GROUP BY g.id, g.name
             ORDER BY count DESC, value ASC
             LIMIT ?
            "
        ),
        library_paths,
    )
    .await
}

/// Run one `(value, count)` facet aggregate, binding `library_paths` into its
/// `IN (…)` placeholder list and [`FACET_LIMIT`] into its trailing `LIMIT ?`.
async fn facet_query(
    pool: &SqlitePool,
    sql: &str,
    library_paths: &[&str],
) -> Result<Vec<FacetCount>, sqlx::Error> {
    let mut q = sqlx::query_as::<_, (String, i64)>(sql);
    for p in library_paths {
        q = q.bind(*p);
    }
    q = q.bind(FACET_LIMIT);
    let rows = q.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(value, count)| FacetCount { value, count })
        .collect())
}

#[cfg(test)]
mod tests;
