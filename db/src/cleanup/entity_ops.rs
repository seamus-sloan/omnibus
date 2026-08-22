//! Low-level, entity-agnostic CRUD shared by [`super::apply`] and
//! [`super::undo`]: link-row read/write parameterized over [`MergeEntity`], the
//! `entity_aliases` door, and the `author_photos` row shape. Encodes no cleanup
//! policy — just the SQL both directions of a merge/split/delete replay.

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

/// Bind-parameter budget shared by the batched split-undo helpers below.
/// [`delete_links_bulk`] chunks *two* `IN (...)` lists in the same
/// statement, so this is sized to leave headroom for both — a chunk of
/// each stays comfortably under SQLite's ~999-parameter cap even when both
/// lists are large.
const SPLIT_UNDO_CHUNK: usize = 450;

/// Resolve every name in `names` to its current `tags.id` in chunked `IN
/// (...)` queries — the batched replacement for a per-atom lookup-then-
/// delete loop, used by [`super::undo::undo_split`] to resolve every split
/// atom to an id in a handful of round trips instead of one per atom. A
/// name with no live row (e.g. already reaped by an intervening
/// [`crate::taxonomy::delete_orphan_taxonomy`]) is silently absent from the
/// result, matching the original per-name `Option` guard it replaces.
pub(super) async fn lookup_tag_ids(
    tx: &mut Transaction<'_, Sqlite>,
    names: &[String],
) -> Result<Vec<i64>, sqlx::Error> {
    let mut ids = Vec::with_capacity(names.len());
    for chunk in names.chunks(SPLIT_UNDO_CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("SELECT id FROM tags WHERE name COLLATE NOCASE IN ({placeholders})");
        let mut q = sqlx::query_scalar::<_, i64>(&sql);
        for name in chunk {
            q = q.bind(name);
        }
        ids.extend(q.fetch_all(&mut **tx).await?);
    }
    Ok(ids)
}

/// Bulk counterpart of [`insert_link`] for entities with no `position`
/// column (tags): one `(book, entity_id)` row per id in `book_ids`, as
/// chunked multi-row `INSERT OR IGNORE` statements rather than one query
/// per book.
pub(super) async fn insert_links_bulk(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    entity_id: i64,
    book_ids: &[i64],
) -> Result<(), sqlx::Error> {
    debug_assert!(
        !entity.has_position(),
        "insert_links_bulk has no position-column support"
    );
    for chunk in book_ids.chunks(SPLIT_UNDO_CHUNK / 2) {
        let placeholders = vec!["(?, ?)"; chunk.len()].join(", ");
        let sql = format!(
            "INSERT OR IGNORE INTO {} (book, {}) VALUES {placeholders}",
            entity.link_table(),
            entity.link_col()
        );
        let mut q = sqlx::query(&sql);
        for &book_id in chunk {
            q = q.bind(book_id).bind(entity_id);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Bulk counterpart of [`delete_link`]: delete every `(book, entity_id)`
/// link row where `book` is in `book_ids` and `entity_id` is in
/// `entity_ids` — used by [`super::undo::undo_split`] to remove exactly the
/// per-book atom links a split added, without the O(books × atoms)
/// lookup-then-delete the naive replay used. Both lists are chunked (not
/// just `book_ids`) so the statement stays bounded even in the pathological
/// case of a split with many atoms, though in practice `entity_ids` is a
/// handful and this degenerates to one outer iteration.
pub(super) async fn delete_links_bulk(
    tx: &mut Transaction<'_, Sqlite>,
    entity: MergeEntity,
    book_ids: &[i64],
    entity_ids: &[i64],
) -> Result<(), sqlx::Error> {
    for entity_chunk in entity_ids.chunks(SPLIT_UNDO_CHUNK) {
        let entity_placeholders = std::iter::repeat_n("?", entity_chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        for book_chunk in book_ids.chunks(SPLIT_UNDO_CHUNK) {
            let book_placeholders = std::iter::repeat_n("?", book_chunk.len())
                .collect::<Vec<_>>()
                .join(", ");
            let sql = format!(
                "DELETE FROM {t} WHERE book IN ({book_placeholders}) AND {c} IN ({entity_placeholders})",
                t = entity.link_table(),
                c = entity.link_col()
            );
            let mut q = sqlx::query(&sql);
            for &book_id in book_chunk {
                q = q.bind(book_id);
            }
            for &entity_id in entity_chunk {
                q = q.bind(entity_id);
            }
            q.execute(&mut **tx).await?;
        }
    }
    Ok(())
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
/// [`write_entity_alias`] for a source name that had **no** alias row before
/// the merge being undone, run so the recreated (independent) source name no
/// longer resolves to the old canonical id.
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

/// The `(canonical_id, created_at)` an `entity_aliases` row held *before* a
/// merge is about to overwrite it via [`write_entity_alias`]'s
/// `ON CONFLICT` — `None` when no such row exists yet. A merge snapshots
/// this ahead of writing its own alias so undo can tell "this name never
/// aliased anything before me" from "this name already pointed somewhere
/// else, from an earlier merge I must not erase".
pub(super) async fn fetch_entity_alias(
    tx: &mut Transaction<'_, Sqlite>,
    kind: CleanupKind,
    alias_name: &str,
) -> Result<Option<(i64, i64)>, sqlx::Error> {
    sqlx::query_as(
        "SELECT canonical_id, created_at FROM entity_aliases WHERE kind = ? AND alias_name = ?",
    )
    .bind(kind.as_str())
    .bind(alias_name)
    .fetch_optional(&mut **tx)
    .await
}

/// Restore an `entity_aliases` row to exactly `(canonical_id, created_at)` —
/// the undo counterpart of [`write_entity_alias`] for a source name whose
/// alias predates the merge being undone, so reversing a later merge can't
/// destroy the mapping an earlier one recorded.
pub(super) async fn restore_entity_alias(
    tx: &mut Transaction<'_, Sqlite>,
    kind: CleanupKind,
    alias_name: &str,
    canonical_id: i64,
    created_at: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT INTO entity_aliases (kind, alias_name, canonical_id, created_at) VALUES (?, ?, ?, ?)
         ON CONFLICT(kind, alias_name) DO UPDATE SET
             canonical_id = excluded.canonical_id, created_at = excluded.created_at",
    )
    .bind(kind.as_str())
    .bind(alias_name)
    .bind(canonical_id)
    .bind(created_at)
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
