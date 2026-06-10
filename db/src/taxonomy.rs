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
resolve_or_insert_simple!(resolve_or_insert_publisher, "publishers", "name");
resolve_or_insert_simple!(resolve_or_insert_language, "languages", "code");

#[cfg(test)]
mod tests;
