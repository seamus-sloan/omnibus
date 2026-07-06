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
    restore_attach_ledger(&mut tx, new_id, &source_uuid, &snap.merged_uuid_rows).await?;

    // The restored `books` row + links + identifiers are all written by
    // now, so the door reconstructs the FTS row from them directly.
    upsert_fts(&mut tx, new_id).await?;

    sqlx::query("UPDATE merge_log SET undone_at = strftime('%s','now') WHERE id = ?")
        .bind(merge_log_id)
        .execute(&mut *tx)
        .await?;

    tx.commit().await?;

    rebuild_target_fts_best_effort(pool, target_id).await?;

    Ok(source_uuid)
}

/// Point the snapshot's pre-merge attachment uuids back at the restored row
/// and drop the source-uuid guard, so future reindexes treat the source's
/// file as its own again rather than a merged attachment of the target.
async fn restore_attach_ledger(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    new_id: i64,
    source_uuid: &str,
    merged_uuid_rows: &[(String, String, String)],
) -> Result<(), sqlx::Error> {
    for (uuid, format, library_path) in merged_uuid_rows {
        sqlx::query(
            "INSERT OR REPLACE INTO merged_uuids (uuid, book_id, format, library_path)
             VALUES (?, ?, ?, ?)",
        )
        .bind(uuid)
        .bind(new_id)
        .bind(format)
        .bind(library_path)
        .execute(&mut **tx)
        .await?;
    }
    sqlx::query("DELETE FROM merged_uuids WHERE uuid = ?")
        .bind(source_uuid)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Post-commit, best-effort rebuild of the merge target's FTS row. The
/// target's row still carries the union (acceptable — links stayed), but a
/// rebuild keeps any override-driven text current. A rebuild failure is
/// logged and swallowed so it can't fail an already-committed undo.
async fn rebuild_target_fts_best_effort(
    pool: &SqlitePool,
    target_id: i64,
) -> Result<(), sqlx::Error> {
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
    Ok(())
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
    // `timestamp` is restored from the snapshot, falling back to now when a
    // pre-0038 snapshot's timestamp couldn't be parsed to an epoch
    // (`de_epoch_flexible` -> None) — the `books` column is nullable since the
    // in-place 0038 conversion dropped its default, so without the COALESCE the
    // restored row would carry a NULL date-added. `last_modified` is stamped
    // now for the same reason. The `pubdate` COALESCE is pre-existing.
    let id: i64 = sqlx::query_scalar(
        "INSERT INTO books
            (uuid, library_id, path, title, sort, author_sort, series_sort, series_index, pubdate,
             timestamp, last_modified, has_cover, description, accent_color, title_norm, author_norm)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, COALESCE(?, datetime('now')),
                 COALESCE(?, strftime('%s','now')), strftime('%s','now'), ?, ?, ?, ?, ?)
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
        // Batch the format lookup, then assign per-format ordinals in Rust in
        // the original `file_ids` order (files that vanished since the merge
        // are absent from the map and skipped, leaving the book fileless).
        let formats_by_file = fetch_file_formats(tx, target_id, file_ids).await?;
        let mut per_format: std::collections::HashMap<String, i64> =
            std::collections::HashMap::new();
        let mut assignments: Vec<(i64, i64)> = Vec::with_capacity(file_ids.len());
        for &fid in file_ids {
            let Some(fmt) = formats_by_file.get(&fid) else {
                continue;
            };
            let ordinal = per_format.entry(fmt.to_uppercase()).or_insert(0);
            assignments.push((fid, *ordinal));
            *ordinal += 1;
        }
        reparent_files_with_ordinals(tx, new_source_id, target_id, &assignments).await?;
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

/// Rows per chunk for [`fetch_file_formats`] / [`reparent_files_with_ordinals`].
/// The SELECT binds one id per row (+1 for `target_id`); the UPDATE binds three
/// (two in the `CASE`, one in the `IN` list). 200 keeps either chunk under
/// SQLite's 999-parameter cap.
const MOVE_BACK_CHUNK: usize = 200;

/// Batch-fetch the `format` for each of `file_ids` still owned by `target_id`,
/// keyed by file id. Files that vanished since the merge are simply absent.
async fn fetch_file_formats(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    target_id: i64,
    file_ids: &[i64],
) -> Result<std::collections::HashMap<i64, String>, sqlx::Error> {
    let mut out = std::collections::HashMap::with_capacity(file_ids.len());
    for chunk in file_ids.chunks(MOVE_BACK_CHUNK) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "SELECT id, format FROM book_files
              WHERE book_id = ? AND id IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (i64, String)>(&sql).bind(target_id);
        for fid in chunk {
            q = q.bind(fid);
        }
        for (id, fmt) in q.fetch_all(&mut **tx).await? {
            out.insert(id, fmt);
        }
    }
    Ok(out)
}

/// Re-parent each `(file_id, ordinal)` onto `new_source_id`, clearing its
/// label, via one `CASE`-based UPDATE per chunk. Equivalent to a per-row
/// `UPDATE … SET book_id, ordinal, label = NULL WHERE id = ? AND book_id = ?`
/// but in a single statement per chunk.
async fn reparent_files_with_ordinals(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    new_source_id: i64,
    target_id: i64,
    assignments: &[(i64, i64)],
) -> Result<(), sqlx::Error> {
    for chunk in assignments.chunks(MOVE_BACK_CHUNK) {
        let cases = std::iter::repeat_n("WHEN ? THEN ?", chunk.len())
            .collect::<Vec<_>>()
            .join(" ");
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE book_files
                SET book_id = ?, ordinal = CASE id {cases} END, label = NULL
              WHERE book_id = ? AND id IN ({placeholders})"
        );
        let mut q = sqlx::query(&sql).bind(new_source_id);
        for (fid, ordinal) in chunk {
            q = q.bind(fid).bind(ordinal);
        }
        q = q.bind(target_id);
        for (fid, _) in chunk {
            q = q.bind(fid);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Restore the source's author links by name in two batched statements
/// (mirroring the sync writer): one `INSERT ... ON CONFLICT` upsert that keeps
/// each name's first non-null sort, then one `INSERT OR IGNORE ... SELECT ...
/// JOIN authors` that resolves ids by name. A name repeated in the snapshot
/// keeps its first (lowest) position — identical to the old per-row
/// `INSERT OR IGNORE` loop, since `books_authors_link` is keyed `(book,
/// author)`.
async fn restore_author_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    authors: &[(String, Option<String>, i64)],
) -> Result<(), sqlx::Error> {
    if authors.is_empty() {
        return Ok(());
    }

    // Distinct names in first-seen order, each mapped to its first non-null
    // sort — matching the old per-row `COALESCE(authors.sort, excluded.sort)`.
    let mut order: Vec<&str> = Vec::new();
    let mut sort_for: std::collections::HashMap<&str, Option<&str>> =
        std::collections::HashMap::new();
    for (name, sort, _) in authors {
        match sort_for.get_mut(name.as_str()) {
            Some(slot) if slot.is_none() => *slot = sort.as_deref(),
            Some(_) => {}
            None => {
                order.push(name);
                sort_for.insert(name, sort.as_deref());
            }
        }
    }

    let author_rows = std::iter::repeat_n("(?, ?)", order.len())
        .collect::<Vec<_>>()
        .join(", ");
    let upsert_sql = format!(
        "INSERT INTO authors (name, sort) VALUES {author_rows} \
         ON CONFLICT(name) DO UPDATE SET sort = COALESCE(authors.sort, excluded.sort)"
    );
    let mut q = sqlx::query(&upsert_sql);
    for name in &order {
        q = q.bind(*name).bind(sort_for[*name]);
    }
    q.execute(&mut **tx).await?;

    // Link rows deduped by name, keeping the first (lowest) position.
    let mut linked: std::collections::HashSet<&str> = std::collections::HashSet::new();
    let link_entries: Vec<(&str, i64)> = authors
        .iter()
        .filter(|(name, _, _)| linked.insert(name))
        .map(|(name, _, pos)| (name.as_str(), *pos))
        .collect();
    let link_rows = std::iter::repeat_n("(?, ?)", link_entries.len())
        .collect::<Vec<_>>()
        .join(", ");
    let link_sql = format!(
        "INSERT OR IGNORE INTO books_authors_link (book, author, position) \
         SELECT ?, a.id, v.column2 \
         FROM (VALUES {link_rows}) AS v \
         JOIN authors a ON a.name = v.column1"
    );
    let mut q = sqlx::query(&link_sql).bind(book_id);
    for (name, pos) in &link_entries {
        q = q.bind(*name).bind(*pos);
    }
    q.execute(&mut **tx).await?;
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
    restore_author_links(tx, book_id, &snap.authors).await?;
    // Series/tags/publishers/languages go through the per-name
    // `resolve_or_insert_*` taxonomy helpers (INSERT OR IGNORE + SELECT id per
    // name). A snapshot restores at most a handful of each on a rare admin
    // undo, and batching would mean a new batch resolver in `taxonomy.rs`; the
    // per-name loop is the accepted tradeoff here.
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
