//! Per-row resolve-or-insert helpers for the single-valued normalized
//! taxonomy tables (`series`, `publishers`, `languages`). Used only by the
//! indexer write path in `sync` — kept module-private at the crate root,
//! exposed to siblings via `pub(crate)`. The multi-valued relations
//! (authors, tags, identifiers) are inserted in batches directly in `sync`.

use sqlx::Transaction;

/// Taxonomy resolve-or-insert helpers. Each returns the row id for the given
/// (case-insensitive) name, inserting a row if one doesn't exist yet.
///
/// The shape is identical for every taxonomy table: `INSERT OR IGNORE INTO
/// <table> (<col>) VALUES (?)` then `SELECT id FROM <table> WHERE <col> = ?`.
/// We use a macro so the table/column appear as compile-time string literals
/// inside `sqlx::query` — no runtime SQL construction with user input.
macro_rules! resolve_or_insert_simple {
    ($name:ident, $table:literal, $col:literal) => {
        pub(crate) async fn $name(
            tx: &mut Transaction<'_, sqlx::Sqlite>,
            value: &str,
        ) -> Result<i64, sqlx::Error> {
            sqlx::query(concat!(
                "INSERT OR IGNORE INTO ",
                $table,
                " (",
                $col,
                ") VALUES (?)",
            ))
            .bind(value)
            .execute(&mut **tx)
            .await?;
            sqlx::query_scalar(concat!("SELECT id FROM ", $table, " WHERE ", $col, " = ?",))
                .bind(value)
                .fetch_one(&mut **tx)
                .await
        }
    };
}

resolve_or_insert_simple!(resolve_or_insert_series, "series", "name");
resolve_or_insert_simple!(resolve_or_insert_tag, "tags", "name");
resolve_or_insert_simple!(resolve_or_insert_publisher, "publishers", "name");
resolve_or_insert_simple!(resolve_or_insert_language, "languages", "code");

/// Delete taxonomy rows (`authors`, `series`, `publishers`, `languages`)
/// no longer referenced by any book, then reap orphaned tags and genres via
/// [`delete_orphan_tags`] and [`delete_orphan_genres`]. Run wherever the last
/// link to a taxonomy row can disappear — the missing-files GC purge, the
/// merge source delete, and the indexer's Changed bucket, which wipes a
/// book's link rows before re-inserting them from the re-parsed file — so an
/// author or series left with zero books doesn't linger, and a book whose
/// `metadata_overrides` row is deleted directly (rather than through the
/// override-editing write paths) doesn't strand a genre it uniquely named.
/// `author_photos` cascades on the author delete; the link tables have no
/// taxonomy-side cascade, so a row is only deletable once its last link is
/// gone, which is exactly the set this targets. Table/column names are
/// compile-time literals, not caller input.
pub(crate) async fn delete_orphan_taxonomy(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
) -> Result<(), sqlx::Error> {
    for (table, link, col) in [
        ("authors", "books_authors_link", "author"),
        ("series", "books_series_link", "series"),
        ("publishers", "books_publishers_link", "publisher"),
        ("languages", "books_languages_link", "language"),
    ] {
        let sql = format!(
            "DELETE FROM {table} \
              WHERE NOT EXISTS (SELECT 1 FROM {link} WHERE {col} = {table}.id)"
        );
        sqlx::query(&sql).execute(&mut **tx).await?;
    }
    delete_orphan_tags(tx).await?;
    delete_orphan_genres(tx).await
}

/// Delete `tags` rows with no book left to justify them: no canonical
/// `books_tags_link` row **and** no subjects-override membership on a live
/// book. Tags need the second clause because override memberships live in
/// `metadata_overrides` JSON rather than the link table (see
/// `materialize_tag_rows`) — a link-only check would reap a tag an override
/// still names. Called from [`delete_orphan_taxonomy`] and directly from the
/// override write paths, which can orphan a tag without touching any link
/// row (a subjects save dropping the tag's last membership, or an override
/// delete). The membership match mirrors `get_tag_cloud`'s override arm:
/// scoped to live `books` rows, filtered to overrides that actually set
/// `$.subjects`, and compared `COLLATE NOCASE` — equivalent to the cloud's
/// `lower()` fold (both are ASCII-only) without recomputing `lower()` per
/// row. A JSON-null or absent `$.subjects` yields no matching `json_each`
/// rows, so such overrides protect nothing.
pub(crate) async fn delete_orphan_tags(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM tags
          WHERE NOT EXISTS (SELECT 1 FROM books_tags_link WHERE tag = tags.id)
            AND NOT EXISTS (
              SELECT 1
                FROM books b
                JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
                JOIN json_each(mo.overrides, '$.subjects') je
               WHERE json_type(mo.overrides, '$.subjects') IS NOT NULL
                 AND je.value = tags.name COLLATE NOCASE
            )",
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Delete `genres` rows no live book's override still names. Genres have no
/// canonical link table (migration `0066`), so this is the whole visibility
/// rule rather than the second half of one — the single clause here is the
/// `metadata_overrides` arm of [`delete_orphan_tags`], with `$.genres` for
/// `$.subjects`. Called from the override write paths (the only way a
/// `genres` row is ever created) and from [`delete_orphan_taxonomy`], which
/// covers the paths that delete a book's `metadata_overrides` row directly.
pub(crate) async fn delete_orphan_genres(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "DELETE FROM genres
          WHERE NOT EXISTS (
            SELECT 1
              FROM books b
              JOIN metadata_overrides mo ON mo.book_uuid = b.uuid
              JOIN json_each(mo.overrides, '$.genres') je
             WHERE json_type(mo.overrides, '$.genres') IS NOT NULL
               AND je.value = genres.name COLLATE NOCASE
          )",
    )
    .execute(&mut **tx)
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests;
