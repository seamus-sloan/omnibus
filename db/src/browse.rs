//! F1.12 browse-all index pages: `/authors` and `/series`. Returns every
//! row (capped at `INDEX_LIMIT`) so the UI's client-side sort/filter has
//! the full list to work with; per-row counts come back override-aware so
//! the index surfaces stay consistent with the discovery-detail reads.

use sqlx::{Row, SqlitePool};

use omnibus_shared::{AuthorSummary, SeriesSummary};

/// Hard cap on rows returned by [`list_authors`] / [`list_series`]. The
/// F1.12 roadmap notes a 5k+ author library is the upper bound we want to
/// keep responsive on a single page; 10k leaves headroom while keeping
/// the JSON envelope under ~1 MB even with the optional accent string.
const INDEX_LIMIT: i64 = 10_000;

/// Return every author with their book count and an optional cover-derived
/// accent, scoped to `library_path`. Empty list when `library_path` does
/// not match a configured library.
///
/// Ordered by name ascending. The UI does its own client-side sort/filter,
/// so this returns the full list up to [`INDEX_LIMIT`] — see
/// [docs/roadmap/1-12-browse-authors-series.md] for the per-library size
/// expectations.
///
/// Accent: the `accent_color` of the first book by this author with a
/// non-null value, by book sort/id order. `NULL` is returned when no book
/// has one, and the UI falls back to the theme accent.
///
/// # Multi-tenancy
///
/// Single-tenant today — every authenticated caller sees the same list.
/// When per-user ACLs land in F4.x this function must accept a `user_id`
/// and join through the access-control table the same way
/// [`crate::discovery::get_author`] will.
pub async fn list_authors(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Vec<AuthorSummary>, sqlx::Error> {
    // TODO(F4.x): scope by `user_id` once per-user ACLs land.
    //
    // F5.1: count uses the effective (override-aware) creator set, not
    // the raw `books_authors_link` rows — otherwise an author whose books
    // were all reassigned through the metadata edit form keeps reporting
    // the canonical count even though `/authors/:id` (override-aware
    // since #153) shows the corrected list. Visibility still requires
    // at least one canonical link in this library so we don't list
    // authors that exist only inside override JSON — they'd have no
    // navigable `authors.id`. The accent subquery is unchanged: accent
    // comes from `books.accent_color` (cover-derived), which overrides
    // can replace via `override_covers/<uuid>` materialisation but never
    // by JSON edit, so the canonical-link lookup is still correct.
    //
    // Same shape as the palette author query (`search_palette`'s B
    // block) so the two index surfaces report consistent counts.
    let rows = sqlx::query(
        r#"
        SELECT a.id, a.name, a.sort,
               (SELECT COUNT(*)
                  FROM books b
                  JOIN libraries l2 ON l2.id = b.library_id
                  LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
                 WHERE l2.path = ?1
                   AND CASE
                         WHEN mo.book_uuid IS NOT NULL
                              AND json_type(mo.overrides, '$.creators') IS NOT NULL
                           THEN EXISTS (
                             SELECT 1 FROM json_each(mo.overrides, '$.creators') je
                              WHERE json_extract(je.value, '$.name') = a.name COLLATE NOCASE
                           )
                         ELSE EXISTS (
                           SELECT 1 FROM books_authors_link bal
                            WHERE bal.book = b.id AND bal.author = a.id
                         )
                       END
               ) AS book_count,
               (SELECT b2.accent_color
                  FROM books_authors_link bal2
                  JOIN books b2 ON b2.id = bal2.book
                  JOIN libraries l2 ON l2.id = b2.library_id
                 WHERE bal2.author = a.id
                   AND l2.path = ?1
                   AND b2.accent_color IS NOT NULL
                 ORDER BY b2.sort, b2.id
                 LIMIT 1) AS accent,
               EXISTS(
                 SELECT 1 FROM author_photos ap
                  WHERE ap.author_id = a.id
                    AND ap.source IN ('manual', 'openlibrary')
                    AND ap.bytes IS NOT NULL
               ) AS has_photo
        FROM authors a
        WHERE EXISTS (
            SELECT 1 FROM books_authors_link bal
              JOIN books b ON b.id = bal.book
              JOIN libraries l ON l.id = b.library_id
             WHERE bal.author = a.id AND l.path = ?1
          )
        ORDER BY COALESCE(a.sort, a.name) COLLATE NOCASE ASC
        LIMIT ?2
        "#,
    )
    .bind(library_path)
    .bind(INDEX_LIMIT)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| AuthorSummary {
            id: r.get("id"),
            name: r.get("name"),
            sort: r.get("sort"),
            book_count: r.get::<i64, _>("book_count") as usize,
            accent: r.get("accent"),
            has_photo: r.get::<i64, _>("has_photo") != 0,
        })
        .collect())
}

/// Return every series with book count, primary author, and an optional
/// accent, scoped to `library_path`.
///
/// `primary_author` is the first creator of the lowest-`series_index`
/// book in the series (with `book.sort, book.id` as deterministic
/// tie-breakers). It can be `None` when every book in the series has only
/// override-supplied creators that haven't been linked to an `authors`
/// row yet — surface a plain text by-line in that case.
///
/// Ordered by name ascending. Capped at [`INDEX_LIMIT`].
///
/// # Multi-tenancy
///
/// Same single-tenant caveat as [`list_authors`].
pub async fn list_series(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Vec<SeriesSummary>, sqlx::Error> {
    // TODO(F4.x): scope by `user_id` once per-user ACLs land.
    //
    // F5.1: same overlay shape as `list_authors` and the palette series
    // query. `overrides.series` (string) drives membership for `book_count`;
    // `overrides.creators[0].name` drives `primary_author` when present so
    // the by-line on the card matches what `/series/:id` (#153 + the
    // empty-index follow-up) shows. Visibility still requires at least one
    // canonical `books_series_link` row in this library — series that
    // exist only as a string inside override JSON have no `series.id` to
    // route to. Accent comes from cover-derived `books.accent_color`,
    // which JSON overrides can't change, so the canonical-link lookup is
    // still correct.
    let rows = sqlx::query(
        r#"
        SELECT s.id, s.name, s.sort,
               (SELECT COUNT(*)
                  FROM books b
                  JOIN libraries l2 ON l2.id = b.library_id
                  LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
                 WHERE l2.path = ?1
                   AND CASE
                         WHEN mo.book_uuid IS NOT NULL
                              AND json_type(mo.overrides, '$.series') IS NOT NULL
                           THEN json_extract(mo.overrides, '$.series') = s.name COLLATE NOCASE
                         ELSE EXISTS (
                           SELECT 1 FROM books_series_link bsl
                            WHERE bsl.book = b.id AND bsl.series = s.id
                         )
                       END
               ) AS book_count,
               (SELECT
                  CASE
                    WHEN mo2.book_uuid IS NOT NULL
                         AND json_type(mo2.overrides, '$.creators') IS NOT NULL
                      THEN json_extract(mo2.overrides, '$.creators[0].name')
                    ELSE (SELECT a.name FROM books_authors_link bal
                            JOIN authors a ON a.id = bal.author
                           WHERE bal.book = b2.id
                           ORDER BY bal.position LIMIT 1)
                  END
                FROM books_series_link bsl2
                  JOIN books b2 ON b2.id = bsl2.book
                  JOIN libraries l2 ON l2.id = b2.library_id
                  LEFT JOIN metadata_overrides mo2 ON mo2.book_uuid = b2.uuid
                 WHERE bsl2.series = s.id AND l2.path = ?1
                 ORDER BY b2.series_index NULLS LAST, b2.sort, b2.id
                 LIMIT 1) AS primary_author,
               (SELECT b3.accent_color
                  FROM books_series_link bsl3
                  JOIN books b3 ON b3.id = bsl3.book
                  JOIN libraries l3 ON l3.id = b3.library_id
                 WHERE bsl3.series = s.id
                   AND l3.path = ?1
                   AND b3.accent_color IS NOT NULL
                 ORDER BY b3.series_index NULLS LAST, b3.sort, b3.id
                 LIMIT 1) AS accent
        FROM series s
        WHERE EXISTS (
            SELECT 1 FROM books_series_link bsl
              JOIN books b ON b.id = bsl.book
              JOIN libraries l ON l.id = b.library_id
             WHERE bsl.series = s.id AND l.path = ?1
          )
        ORDER BY COALESCE(s.sort, s.name) COLLATE NOCASE ASC
        LIMIT ?2
        "#,
    )
    .bind(library_path)
    .bind(INDEX_LIMIT)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| SeriesSummary {
            id: r.get("id"),
            name: r.get("name"),
            sort: r.get("sort"),
            book_count: r.get::<i64, _>("book_count") as usize,
            primary_author: r.get("primary_author"),
            accent: r.get("accent"),
        })
        .collect())
}
