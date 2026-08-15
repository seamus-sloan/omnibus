//! Low-level, entity-agnostic CRUD used by both [`super::apply`] and
//! [`super::undo`]: link-row read/write parameterized over [`MergeEntity`],
//! the `entity_aliases` door, and the `author_photos` row shape. Split out
//! of `apply.rs` once it crossed the file-shape soft cap — none of this
//! encodes cleanup policy (priority order, what gets snapshotted); it's
//! just the SQL both directions of a merge/split/delete replay.

use omnibus_shared::CleanupKind;
use sqlx::{Sqlite, Transaction};

use super::snapshot::{BookLink, PhotoSnapshot};
use super::MergeEntity;

/// `SELECT name, sort` for the entity, projecting a literal `NULL` for the
/// `sort` column when `entity` has none (tags).
pub(super) async fn fetch_name_sort(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    id: i64,
) -> Result<Option<(String, Option<String>)>, sqlx::Error> {
    let sort_expr = if entity.has_sort() { "sort" } else { "NULL" };
    let sql = format!(
        "SELECT name, {sort_expr} FROM {} WHERE id = ?",
        entity.table()
    );
    sqlx::query_as(&sql)
        .bind(id)
        .fetch_optional(&mut **tx)
        .await
}

/// Every `(book_id, position)` link row for `id`, `position` `None` for
/// entities with no such column.
pub(super) async fn fetch_links(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    id: i64,
) -> Result<Vec<BookLink>, sqlx::Error> {
    if entity.has_position() {
        let sql = format!(
            "SELECT book, position FROM {} WHERE {} = ?",
            entity.link_table(),
            entity.link_col()
        );
        let rows: Vec<(i64, i64)> = sqlx::query_as(&sql).bind(id).fetch_all(&mut **tx).await?;
        Ok(rows
            .into_iter()
            .map(|(book_id, position)| BookLink {
                book_id,
                position: Some(position),
            })
            .collect())
    } else {
        let sql = format!(
            "SELECT book FROM {} WHERE {} = ?",
            entity.link_table(),
            entity.link_col()
        );
        let rows: Vec<i64> = sqlx::query_scalar(&sql)
            .bind(id)
            .fetch_all(&mut **tx)
            .await?;
        Ok(rows
            .into_iter()
            .map(|book_id| BookLink {
                book_id,
                position: None,
            })
            .collect())
    }
}

/// Just the linked book ids, for a set membership check.
pub(super) async fn fetch_linked_book_ids(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    id: i64,
) -> Result<Vec<i64>, sqlx::Error> {
    let sql = format!(
        "SELECT book FROM {} WHERE {} = ?",
        entity.link_table(),
        entity.link_col()
    );
    sqlx::query_scalar(&sql).bind(id).fetch_all(&mut **tx).await
}

/// `INSERT OR IGNORE` every one of `source_id`'s link rows onto
/// `canonical_id` — a book already linked to both survives untouched.
pub(super) async fn move_links(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    source_id: i64,
    canonical_id: i64,
) -> Result<(), sqlx::Error> {
    let sql = if entity.has_position() {
        format!(
            "INSERT OR IGNORE INTO {t} (book, {c}, position)
             SELECT book, ?, position FROM {t} WHERE {c} = ?",
            t = entity.link_table(),
            c = entity.link_col()
        )
    } else {
        format!(
            "INSERT OR IGNORE INTO {t} (book, {c})
             SELECT book, ? FROM {t} WHERE {c} = ?",
            t = entity.link_table(),
            c = entity.link_col()
        )
    };
    sqlx::query(&sql)
        .bind(canonical_id)
        .bind(source_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Delete every link row naming `id`.
pub(super) async fn delete_links(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    id: i64,
) -> Result<(), sqlx::Error> {
    let sql = format!(
        "DELETE FROM {} WHERE {} = ?",
        entity.link_table(),
        entity.link_col()
    );
    sqlx::query(&sql).bind(id).execute(&mut **tx).await?;
    Ok(())
}

/// Insert one `(book_id, entity_id[, position])` link row, tolerating an
/// already-existing row.
pub(super) async fn insert_link(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    book_id: i64,
    entity_id: i64,
    position: Option<i64>,
) -> Result<(), sqlx::Error> {
    if entity.has_position() {
        let sql = format!(
            "INSERT OR IGNORE INTO {} (book, {}, position) VALUES (?, ?, ?)",
            entity.link_table(),
            entity.link_col()
        );
        sqlx::query(&sql)
            .bind(book_id)
            .bind(entity_id)
            .bind(position.unwrap_or(0))
            .execute(&mut **tx)
            .await?;
    } else {
        let sql = format!(
            "INSERT OR IGNORE INTO {} (book, {}) VALUES (?, ?)",
            entity.link_table(),
            entity.link_col()
        );
        sqlx::query(&sql)
            .bind(book_id)
            .bind(entity_id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Delete one `(book_id, entity_id)` link row.
pub(super) async fn delete_link(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    book_id: i64,
    entity_id: i64,
) -> Result<(), sqlx::Error> {
    let sql = format!(
        "DELETE FROM {} WHERE book = ? AND {} = ?",
        entity.link_table(),
        entity.link_col()
    );
    sqlx::query(&sql)
        .bind(book_id)
        .bind(entity_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Delete the entity's own row (link rows must already be gone).
pub(super) async fn delete_entity_row(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    id: i64,
) -> Result<(), sqlx::Error> {
    let sql = format!("DELETE FROM {} WHERE id = ?", entity.table());
    sqlx::query(&sql).bind(id).execute(&mut **tx).await?;
    Ok(())
}

/// Backfill `sort` on a row that currently has none — [`clear_sort`] is the
/// undo counterpart.
pub(super) async fn set_sort(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    id: i64,
    sort: &str,
) -> Result<(), sqlx::Error> {
    let sql = format!("UPDATE {} SET sort = ? WHERE id = ?", entity.table());
    sqlx::query(&sql)
        .bind(sort)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Reset `sort` to NULL — the undo counterpart of [`set_sort`]'s backfill.
pub(super) async fn clear_sort(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    id: i64,
) -> Result<(), sqlx::Error> {
    let sql = format!("UPDATE {} SET sort = NULL WHERE id = ?", entity.table());
    sqlx::query(&sql).bind(id).execute(&mut **tx).await?;
    Ok(())
}

/// Resolve-or-insert an entity by name, tolerating a UNIQUE collision with a
/// row created since the source was deleted: if a same-named row already
/// exists its id is reused (and its `sort` backfilled from `sort` only if
/// currently NULL, so a legitimately different existing row isn't
/// clobbered).
pub(super) async fn recreate_entity(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    name: &str,
    sort: Option<&str>,
) -> Result<i64, sqlx::Error> {
    if entity.has_sort() {
        let ins = format!(
            "INSERT OR IGNORE INTO {} (name, sort) VALUES (?, ?)",
            entity.table()
        );
        sqlx::query(&ins)
            .bind(name)
            .bind(sort)
            .execute(&mut **tx)
            .await?;
        if let Some(sort) = sort {
            let upd = format!(
                "UPDATE {} SET sort = COALESCE(sort, ?) WHERE name = ? COLLATE NOCASE",
                entity.table()
            );
            sqlx::query(&upd)
                .bind(sort)
                .bind(name)
                .execute(&mut **tx)
                .await?;
        }
    } else {
        let ins = format!("INSERT OR IGNORE INTO {} (name) VALUES (?)", entity.table());
        sqlx::query(&ins).bind(name).execute(&mut **tx).await?;
    }
    let sel = format!(
        "SELECT id FROM {} WHERE name = ? COLLATE NOCASE",
        entity.table()
    );
    sqlx::query_scalar(&sel)
        .bind(name)
        .fetch_one(&mut **tx)
        .await
}

/// Look up a tag's current id by name, for undoing a split's per-atom links.
pub(super) async fn lookup_tag_id(
    tx: &mut Transaction<'_, Sqlite>,
    name: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar("SELECT id FROM tags WHERE name = ? COLLATE NOCASE")
        .bind(name)
        .fetch_optional(&mut **tx)
        .await
}

/// Record that `alias_name` (a name a merge just absorbed) now resolves to
/// `canonical_id` — consulted by a future reindex-resurrection guard.
pub(super) async fn write_entity_alias(
    tx: &mut Transaction<'_, Sqlite>,
    kind: CleanupKind,
    alias_name: &str,
    canonical_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO entity_aliases (kind, alias_name, canonical_id) VALUES (?, ?, ?)
         ON CONFLICT(kind, alias_name) DO UPDATE SET
             canonical_id = excluded.canonical_id, created_at = strftime('%s','now')",
    )
    .bind(kind.as_str())
    .bind(alias_name)
    .bind(canonical_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Remove an `entity_aliases` row — the undo counterpart of
/// [`write_entity_alias`], run when a merge is reversed so the recreated
/// (independent) source name no longer resolves to the old canonical id.
pub(super) async fn delete_entity_alias(
    tx: &mut Transaction<'_, Sqlite>,
    kind: CleanupKind,
    alias_name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM entity_aliases WHERE kind = ? AND alias_name = ?")
        .bind(kind.as_str())
        .bind(alias_name)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// `(source, url, mime, bytes)` — the raw `author_photos` row shape.
type PhotoRow = (String, Option<String>, Option<String>, Option<Vec<u8>>);

pub(super) async fn load_photo_snapshot(
    tx: &mut Transaction<'_, Sqlite>,
    author_id: i64,
) -> Result<Option<PhotoSnapshot>, sqlx::Error> {
    let row: Option<PhotoRow> =
        sqlx::query_as("SELECT source, url, mime, bytes FROM author_photos WHERE author_id = ?")
            .bind(author_id)
            .fetch_optional(&mut **tx)
            .await?;
    Ok(row.map(|(source, url, mime, bytes)| PhotoSnapshot {
        source,
        url,
        mime,
        bytes_b64: PhotoSnapshot::encode(bytes.as_deref()),
    }))
}

pub(super) async fn write_photo(
    tx: &mut Transaction<'_, Sqlite>,
    author_id: i64,
    photo: &PhotoSnapshot,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO author_photos (author_id, source, url, mime, bytes, fetched_at)
              VALUES (?, ?, ?, ?, ?, strftime('%s','now'))
         ON CONFLICT(author_id) DO UPDATE SET
              source = excluded.source, url = excluded.url, mime = excluded.mime,
              bytes = excluded.bytes, fetched_at = excluded.fetched_at",
    )
    .bind(author_id)
    .bind(&photo.source)
    .bind(&photo.url)
    .bind(&photo.mime)
    .bind(photo.decode())
    .execute(&mut **tx)
    .await?;
    Ok(())
}

async fn delete_photo(tx: &mut Transaction<'_, Sqlite>, author_id: i64) -> Result<(), sqlx::Error> {
    sqlx::query("DELETE FROM author_photos WHERE author_id = ?")
        .bind(author_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Restore the canonical author's photo row to `before` — `None` deletes
/// whatever the merge's photo reconciliation may have written.
pub(super) async fn restore_canonical_photo(
    tx: &mut Transaction<'_, Sqlite>,
    canonical_id: i64,
    before: Option<&PhotoSnapshot>,
) -> Result<(), sqlx::Error> {
    match before {
        Some(p) => write_photo(tx, canonical_id, p).await,
        None => delete_photo(tx, canonical_id).await,
    }
}

/// Insert the `cleanup_log` audit row and return its id — the undo handle.
pub(super) async fn write_cleanup_log<T: serde::Serialize>(
    tx: &mut Transaction<'_, Sqlite>,
    suggestion_id: Option<i64>,
    kind: CleanupKind,
    action: omnibus_shared::CleanupAction,
    snapshot: &T,
    applied_by: Option<i64>,
) -> Result<i64, super::apply::CleanupApplyError> {
    let json = serde_json::to_string(snapshot)?;
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO cleanup_log (suggestion_id, kind, action, snapshot_json, applied_by)
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(suggestion_id)
    .bind(kind.as_str())
    .bind(action.as_str())
    .bind(json)
    .bind(applied_by)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}
