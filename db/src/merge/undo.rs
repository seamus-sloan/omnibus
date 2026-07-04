//! Undo for [`super::merge_books`]: recreate the absorbed source book
//! from its `merge_log` snapshot and move its file rows back.

use sqlx::{SqlitePool, Transaction};

use crate::settings::upsert_library;
use crate::sync::upsert_fts;
use crate::taxonomy::{
    resolve_or_insert_language, resolve_or_insert_publisher, resolve_or_insert_series,
    resolve_or_insert_tag,
};

use super::snapshot::SourceSnapshot;
use super::MergeError;

/// Reverse a merge: recreate the source `books` row from the snapshot,
/// move the snapshot's file formats back off the target, restore links
/// by name, and clear the source-uuid reindex guard. Returns the
/// restored book's uuid.
///
/// Deliberate asymmetries: progress and history stay on the target;
/// links unioned into the target stay there; merged override values stay
/// on the target. If a moved file row was deleted in the meantime (file
/// removed from disk), the restored source comes back **fileless** — a
/// legal state — rather than failing.
pub async fn undo_merge(pool: &SqlitePool, merge_log_id: i64) -> Result<String, MergeError> {
    let mut tx = pool.begin().await?;

    let row: Option<(i64, String, String, Option<i64>)> = sqlx::query_as(
        "SELECT target_book_id, source_uuid, source_metadata, undone_at
           FROM merge_log WHERE id = ?",
    )
    .bind(merge_log_id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((target_id, source_uuid, json, undone_at)) = row else {
        return Err(MergeError::LogNotFound);
    };
    if undone_at.is_some() {
        return Err(MergeError::AlreadyUndone);
    }
    let snap: SourceSnapshot = serde_json::from_str(&json)?;

    let new_id = recreate_source_row(&mut tx, &snap).await?;
    move_files_back(
        &mut tx,
        target_id,
        new_id,
        &snap.moved_file_ids,
        &snap.moved_formats,
    )
    .await?;
    restore_links(&mut tx, new_id, &snap).await?;
    restore_identifiers(&mut tx, new_id, &snap).await?;

    // Point pre-merge attachment uuids back at the restored row and drop
    // the source-uuid guard so future reindexes treat the file as the
    // source's own again.
    for (uuid, format, library_path) in &snap.merged_uuid_rows {
        sqlx::query(
            "INSERT OR REPLACE INTO merged_uuids (uuid, book_id, format, library_path)
             VALUES (?, ?, ?, ?)",
        )
        .bind(uuid)
        .bind(new_id)
        .bind(format)
        .bind(library_path)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query("DELETE FROM merged_uuids WHERE uuid = ?")
        .bind(&source_uuid)
        .execute(&mut *tx)
        .await?;

    // The restored `books` row + links + identifiers are all written by
    // now, so the door reconstructs the FTS row from them directly.
    upsert_fts(&mut tx, new_id).await?;

    sqlx::query("UPDATE merge_log SET undone_at = strftime('%s','now') WHERE id = ?")
        .bind(merge_log_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    // Post-commit, best-effort: the target's FTS row still carries the
    // union (acceptable — links stayed), but rebuild it anyway so any
    // override-driven text is current.
    let target_uuid: Option<String> = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(target_id)
        .fetch_optional(pool)
        .await?;
    if let Some(uuid) = target_uuid {
        if let Err(e) = crate::metadata_overrides::rebuild_fts_for_books_batch(
            pool,
            std::slice::from_ref(&uuid),
        )
        .await
        {
            tracing::warn!(error = %e, uuid = %uuid, "undo_merge: target FTS rebuild failed");
        }
    }

    Ok(source_uuid)
}

/// Recreate the source `books` row from the snapshot. The original uuid
/// is free to reuse — the `merged_uuids` guard kept reindexes from
/// resurrecting it.
async fn recreate_source_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    snap: &SourceSnapshot,
) -> Result<i64, MergeError> {
    let library_id = upsert_library(tx, &snap.library_path)
        .await
        .map_err(|e| match e {
            crate::settings::SettingsError::Db(inner) => MergeError::Db(inner),
            // `upsert_library` never returns `Validation` — the arm exists only to
            // keep the match exhaustive after `SettingsError` grew a validation
            // variant for `set_hardcover_api_key`.
            crate::settings::SettingsError::Validation(msg) => MergeError::Db(
                sqlx::Error::Protocol(format!("unexpected settings validation error: {msg}")),
            ),
        })?;
    // `timestamp` is restored from the snapshot (epoch, migration 0038);
    // `last_modified` is stamped now, set explicitly because the in-place
    // 0038 conversion dropped the `books` column default. The `pubdate`
    // COALESCE fallback is pre-existing and unrelated to F11.
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books
            (uuid, library_id, path, title, sort, author_sort, series_sort, series_index, pubdate,
             timestamp, last_modified, has_cover, description, accent_color, title_norm, author_norm)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, COALESCE(?, datetime('now')),
                 ?, strftime('%s','now'), ?, ?, ?, ?, ?)
         RETURNING id",
    )
    .bind(&snap.uuid)
    .bind(library_id)
    .bind(&snap.path)
    .bind(&snap.title)
    .bind(&snap.sort)
    .bind(&snap.author_sort)
    .bind(&snap.series_sort)
    .bind(snap.series_index)
    .bind(&snap.pubdate)
    .bind(snap.timestamp)
    .bind(snap.has_cover)
    .bind(&snap.description)
    .bind(&snap.accent_color)
    .bind(&snap.title_norm)
    .bind(&snap.author_norm)
    .fetch_one(&mut **tx)
    .await?;
    Ok(id)
}

/// Move the snapshot's files back from the target. Uses file IDs when
/// available (new snapshots); falls back to format matching for old
/// snapshots created before same-format merges were allowed. Rows that
/// vanished since the merge are skipped — the restored book is then
/// fileless. Moved files get sequential ordinals per format and their
/// label cleared.
async fn move_files_back(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    target_id: i64,
    new_source_id: i64,
    file_ids: &[i64],
    formats: &[String],
) -> Result<(), sqlx::Error> {
    if !file_ids.is_empty() {
        // Fetch format for each file so we can assign per-format ordinals.
        let mut per_format: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        for &fid in file_ids {
            let fmt: Option<String> =
                sqlx::query_scalar("SELECT format FROM book_files WHERE id = ? AND book_id = ?")
                    .bind(fid)
                    .bind(target_id)
                    .fetch_optional(&mut **tx)
                    .await?;
            let Some(fmt) = fmt else { continue };
            let ordinal = per_format.entry(fmt.to_uppercase()).or_insert(0);
            sqlx::query(
                "UPDATE book_files SET book_id = ?1, ordinal = ?2, label = NULL
                  WHERE id = ?3 AND book_id = ?4",
            )
            .bind(new_source_id)
            .bind(*ordinal)
            .bind(fid)
            .bind(target_id)
            .execute(&mut **tx)
            .await?;
            *ordinal += 1;
        }
    } else {
        // Legacy snapshot without file IDs — move by format.
        for fmt in formats {
            sqlx::query("UPDATE book_files SET book_id = ?1 WHERE book_id = ?2 AND format = ?3")
                .bind(new_source_id)
                .bind(target_id)
                .bind(fmt)
                .execute(&mut **tx)
                .await?;
        }
    }
    Ok(())
}

/// Restore the source's link rows by **name** — taxonomy ids may have
/// been garbage-collected between merge and undo. The unioned copies on
/// the target are left in place.
async fn restore_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    snap: &SourceSnapshot,
) -> Result<(), sqlx::Error> {
    for (name, sort, position) in &snap.authors {
        sqlx::query(
            "INSERT INTO authors (name, sort) VALUES (?, ?)
             ON CONFLICT(name) DO UPDATE SET sort = COALESCE(authors.sort, excluded.sort)",
        )
        .bind(name)
        .bind(sort)
        .execute(&mut **tx)
        .await?;
        sqlx::query(
            "INSERT OR IGNORE INTO books_authors_link (book, author, position)
             SELECT ?, a.id, ? FROM authors a WHERE a.name = ?",
        )
        .bind(book_id)
        .bind(position)
        .bind(name)
        .execute(&mut **tx)
        .await?;
    }
    for name in &snap.series {
        let id = resolve_or_insert_series(tx, name).await?;
        sqlx::query("INSERT OR IGNORE INTO books_series_link (book, series) VALUES (?, ?)")
            .bind(book_id)
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    for name in &snap.tags {
        let id = resolve_or_insert_tag(tx, name).await?;
        sqlx::query("INSERT OR IGNORE INTO books_tags_link (book, tag) VALUES (?, ?)")
            .bind(book_id)
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    for name in &snap.publishers {
        let id = resolve_or_insert_publisher(tx, name).await?;
        sqlx::query("INSERT OR IGNORE INTO books_publishers_link (book, publisher) VALUES (?, ?)")
            .bind(book_id)
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    for code in &snap.languages {
        let id = resolve_or_insert_language(tx, code).await?;
        sqlx::query("INSERT OR IGNORE INTO books_languages_link (book, language) VALUES (?, ?)")
            .bind(book_id)
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Restore the source's identifier rows. The target keeps its unioned
/// copies (per-scheme target-wins from the merge).
async fn restore_identifiers(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    snap: &SourceSnapshot,
) -> Result<(), sqlx::Error> {
    for (scheme, value) in &snap.identifiers {
        sqlx::query(
            "INSERT OR IGNORE INTO book_identifiers (book_id, scheme, value) VALUES (?, ?, ?)",
        )
        .bind(book_id)
        .bind(scheme)
        .bind(value)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}
