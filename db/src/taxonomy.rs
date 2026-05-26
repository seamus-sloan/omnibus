//! Per-row resolve-or-insert helpers for the normalized taxonomy tables
//! (`series`, `tags`, `publishers`, `languages`, `authors`). Used only by
//! the indexer write path in `sync` — kept module-private at the crate
//! root, exposed to siblings via `pub(crate)`.

use sqlx::Transaction;

/// Taxonomy resolve-or-insert helpers. Each returns the row id for the given
/// (case-insensitive) name, inserting a row if one doesn't exist yet.
///
/// The shape is identical for every taxonomy table: `INSERT OR IGNORE INTO
/// <table> (<col>) VALUES (?)` then `SELECT id FROM <table> WHERE <col> = ?`.
/// We use a macro so the table/column appear as compile-time string literals
/// inside `sqlx::query` — no runtime SQL construction with user input.
/// `authors` is hand-written because it carries an extra `sort` column whose
/// `ON CONFLICT(name) DO UPDATE SET sort = COALESCE(...)` semantics don't fit
/// the simple `INSERT OR IGNORE` template.
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

/// Resolve an author name to its row id, inserting a new row if necessary.
///
/// Returns `Ok(None)` when `name` is in `ignored_authors` — the
/// F5.9-lite blocklist that keeps the next reindex from silently
/// re-creating an admin-deleted junk-author row. Callers (see
/// `insert_metadata_links`) must tolerate `None` by skipping the
/// contributor entirely; that "skip on hit" is what makes admin author
/// deletes durable across reindexes.
pub(crate) async fn resolve_or_insert_author(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    name: &str,
    sort: Option<&str>,
) -> Result<Option<i64>, sqlx::Error> {
    let blocked: Option<i64> =
        sqlx::query_scalar("SELECT 1 FROM ignored_authors WHERE name = ? LIMIT 1")
            .bind(name)
            .fetch_optional(&mut **tx)
            .await?;
    if blocked.is_some() {
        return Ok(None);
    }
    sqlx::query(
        "INSERT INTO authors (name, sort) VALUES (?, ?)
         ON CONFLICT(name) DO UPDATE SET sort = COALESCE(authors.sort, excluded.sort)",
    )
    .bind(name)
    .bind(sort)
    .execute(&mut **tx)
    .await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
        .bind(name)
        .fetch_one(&mut **tx)
        .await?;
    Ok(Some(id))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pool::init_db;

    #[tokio::test]
    async fn resolve_or_insert_author_inserts_when_blocklist_is_empty() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let mut tx = pool.begin().await.unwrap();
        let id = resolve_or_insert_author(&mut tx, "Ada Lovelace", None)
            .await
            .unwrap();
        tx.commit().await.unwrap();
        assert!(id.is_some(), "empty blocklist should not skip insertion");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE name = ?")
            .bind("Ada Lovelace")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 1);
    }

    #[tokio::test]
    async fn resolve_or_insert_author_returns_none_when_blocked() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        sqlx::query("INSERT INTO ignored_authors(name) VALUES (?)")
            .bind("calibre (8.0.0)")
            .execute(&pool)
            .await
            .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let id = resolve_or_insert_author(&mut tx, "calibre (8.0.0)", None)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert!(id.is_none(), "blocked name must skip insertion");
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors WHERE name = ?")
            .bind("calibre (8.0.0)")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(count, 0, "no authors row should have been created");
    }

    #[tokio::test]
    async fn resolve_or_insert_author_blocklist_match_is_case_insensitive() {
        // `ignored_authors.name` is declared COLLATE NOCASE so a junk
        // contributor that comes back from the OPF with different casing
        // still gets skipped.
        let pool = init_db("sqlite::memory:").await.unwrap();
        sqlx::query("INSERT INTO ignored_authors(name) VALUES (?)")
            .bind("Calibre (8.0.0)")
            .execute(&pool)
            .await
            .unwrap();

        let mut tx = pool.begin().await.unwrap();
        let id = resolve_or_insert_author(&mut tx, "CALIBRE (8.0.0)", None)
            .await
            .unwrap();
        tx.commit().await.unwrap();

        assert!(id.is_none());
    }
}
