//! Full-library facet-count aggregate (F5b). [`library_facets`] tallies the
//! sidebar facets (authors / series / formats / tags) across the *entire*
//! configured library in one grouped query each, so the landing sidebar stays
//! correct even though the list itself is keyset-paginated and the client only
//! holds one page. Counts are deliberately over the unfiltered library —
//! mirroring the former client-side `facet_counts`, which tallied the full
//! hydrated list regardless of the active filter selection.

use sqlx::SqlitePool;

use omnibus_shared::{FacetCount, FacetCounts};

/// Per-facet book counts for `library_paths`, ordered by count descending then
/// value ascending (the order the sidebar renders). Empty `library_paths`
/// returns empty facets. Books with no backing file (F2 ghosts) are excluded,
/// matching the list/count read paths.
pub async fn library_facets(
    pool: &SqlitePool,
    library_paths: &[&str],
) -> Result<FacetCounts, super::BooksError> {
    if library_paths.is_empty() {
        return Ok(FacetCounts::default());
    }
    let ph = super::page::placeholders(library_paths.len());

    let authors = facet_query(
        pool,
        &format!(
            r"
            SELECT a.name AS value, COUNT(*) AS count
              FROM books_authors_link bal
              JOIN authors a ON a.id = bal.author
              JOIN books b ON b.id = bal.book
              JOIN scan_roots l ON l.id = b.library_id
             WHERE l.path IN ({ph})
               AND EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
             GROUP BY a.name
             ORDER BY count DESC, value ASC
            "
        ),
        library_paths,
    )
    .await?;

    let series = facet_query(
        pool,
        &format!(
            r"
            SELECT s.name AS value, COUNT(*) AS count
              FROM books_series_link bsl
              JOIN series s ON s.id = bsl.series
              JOIN books b ON b.id = bsl.book
              JOIN scan_roots l ON l.id = b.library_id
             WHERE l.path IN ({ph})
               AND s.name <> ''
               AND EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
             GROUP BY s.name
             ORDER BY count DESC, value ASC
            "
        ),
        library_paths,
    )
    .await?;

    // The JOIN to `book_files` already excludes ghosts (a row with no file
    // contributes none), so no extra EXISTS is needed here. `(book_id, format)`
    // is unique, so COUNT(DISTINCT b.id) per lowercased format equals the
    // number of books carrying it.
    let formats = facet_query(
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
            "
        ),
        library_paths,
    )
    .await?;

    let tags = facet_query(
        pool,
        &format!(
            r"
            SELECT t.name AS value, COUNT(*) AS count
              FROM books_tags_link btl
              JOIN tags t ON t.id = btl.tag
              JOIN books b ON b.id = btl.book
              JOIN scan_roots l ON l.id = b.library_id
             WHERE l.path IN ({ph})
               AND t.name <> ''
               AND EXISTS (SELECT 1 FROM book_files bf WHERE bf.book_id = b.id)
             GROUP BY t.name
             ORDER BY count DESC, value ASC
            "
        ),
        library_paths,
    )
    .await?;

    Ok(FacetCounts {
        authors,
        series,
        formats,
        tags,
    })
}

/// Run one `(value, count)` facet aggregate, binding `library_paths` into its
/// `IN (…)` placeholder list.
async fn facet_query(
    pool: &SqlitePool,
    sql: &str,
    library_paths: &[&str],
) -> Result<Vec<FacetCount>, sqlx::Error> {
    let mut q = sqlx::query_as::<_, (String, i64)>(sql);
    for p in library_paths {
        q = q.bind(*p);
    }
    let rows = q.fetch_all(pool).await?;
    Ok(rows
        .into_iter()
        .map(|(value, count)| FacetCount { value, count })
        .collect())
}

#[cfg(test)]
mod tests;
