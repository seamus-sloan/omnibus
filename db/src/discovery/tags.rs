//! Tag-cloud read: the global tag list with per-tag book counts. Counts
//! use the merged (override-aware) subject set so user-edited subjects on
//! a book are reflected here; a tag is visible only while at least one
//! book effectively carries it — via a canonical `books_tags_link` row or
//! an override membership (the override write path materializes a `tags`
//! row for every override subject). A tag with zero effective books never
//! renders.

use omnibus_shared::TagWeight;
use sqlx::{Row, SqlitePool};

use super::DiscoveryError;

/// Maximum number of tags returned from [`get_tag_cloud`]. Caps the
/// payload so a Calibre dump with 10k+ unique subjects can't blow up
/// the client or stall the SQLite pool on serialization.
const TAG_CLOUD_LIMIT: i64 = 500;

/// Return up to [`TAG_CLOUD_LIMIT`] tags with their book counts, ordered
/// by count descending then name ascending. Used by the tag cloud page.
///
/// Currently returns results across all users (single-tenant); scoping to a
/// per-user `user_id` is a future extension, not yet needed.
pub async fn get_tag_cloud(pool: &SqlitePool) -> Result<Vec<TagWeight>, DiscoveryError> {
    // Counts use the effective (override-aware) subject set, not the raw
    // `books_tags_link` rows — `overrides.subjects` replaces a book's
    // canonical tag list wholesale when Some. Visibility requires an
    // effective membership: the override write path materializes a `tags`
    // row for every override subject (`materialize_tag_rows`), so a tag
    // created via the inline table editor feeds this cloud — and therefore
    // the editor's own autocomplete pool — without polluting the scanned
    // link table.
    //
    // The `effective` CTE is `AS MATERIALIZED` (like `matches` in
    // `fetch_search_rows`) so the override extraction — `json_each` over
    // every `metadata_overrides` row — is built once per call rather than
    // re-scanned per tag. The two UNION arms are disjoint on the key
    // columns (arm 1 sets `tag_name = NULL`, arm 2 sets `tag_id = NULL`)
    // and a book with a subjects override is excluded from arm 1, so no
    // single book reaches a tag through both arms — the OR-join in the
    // final `GROUP BY` sums without double-counting. The inner JOIN is the
    // visibility rule: a tag with zero effective members — e.g. every
    // canonical book overridden away — is hidden, per "a tag with no books
    // doesn't exist". (Its `tags` row and canonical links survive so
    // revert-to-scanned still works; rows with no references at all are
    // physically reaped write-side by `delete_orphan_tags`.) Override
    // names are folded with `lower()` on both sides of the match so a
    // case-variant override ("fiction" against a canonical "Fiction" row)
    // counts toward — and keeps visible — the NOCASE-unique `tags` row it
    // already deduplicated into at materialize time.
    let rows = sqlx::query(
        r#"WITH effective AS MATERIALIZED (
             -- (1) Canonical tag memberships with no subjects override.
             SELECT btl.tag AS tag_id, NULL AS tag_name, btl.book AS book_id
               FROM books_tags_link btl
               JOIN books b ON b.id = btl.book
               LEFT JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
              WHERE mo.book_uuid IS NULL
                 OR json_type(mo.overrides, '$.subjects') IS NULL
             UNION
             -- (2) Override-extracted subject memberships. UNION (not ALL)
             -- dedupes duplicate subject strings within one override array
             -- so a book with `["fiction","fiction"]` still counts once,
             -- matching the prior `EXISTS` semantics.
             SELECT NULL AS tag_id, lower(je.value) AS tag_name, b.id AS book_id
               FROM books b
               JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
               JOIN json_each(mo.overrides, '$.subjects') je
              WHERE json_type(mo.overrides, '$.subjects') IS NOT NULL
           )
           SELECT t.name, COUNT(e.book_id) AS cnt
           FROM tags t
           JOIN effective e
             ON e.tag_id = t.id OR e.tag_name = lower(t.name)
           GROUP BY t.id, t.name
           ORDER BY cnt DESC, t.name ASC
           LIMIT ?"#,
    )
    .bind(TAG_CLOUD_LIMIT)
    .fetch_all(pool)
    .await?;

    Ok(rows
        .iter()
        .map(|r| TagWeight {
            name: r.get("name"),
            count: usize::try_from(r.get::<i64, _>("cnt")).unwrap_or(0),
        })
        .collect())
}
