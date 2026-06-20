//! The merge transaction: move every satellite row from the source book
//! to the target, snapshot the source into `merge_log`, and delete the
//! source row — all-or-nothing.

use sqlx::{SqlitePool, Transaction};

use omnibus_shared::MetadataOverrides;

use crate::covers::{find_cover_file, write_cover_file};
use crate::sync::delete_fts;

use super::snapshot::{build_snapshot, SourceSnapshot};
use super::{MergeError, MergeOutcome};

/// Merge the book identified by `source_uuid` into the one identified by
/// `target_uuid`. The target's scanned metadata wins; the source's
/// files, links, identifiers, progress, and history move over; the
/// source row is deleted and snapshotted into `merge_log` for undo.
///
/// Same-format files are allowed: if both books carry M4B files, the
/// source's files are appended after the target's (ordinal offset).
pub async fn merge_books(
    pool: &SqlitePool,
    source_uuid: &str,
    target_uuid: &str,
    merged_by: Option<i64>,
) -> Result<MergeOutcome, MergeError> {
    if source_uuid == target_uuid {
        return Err(MergeError::SameBook);
    }
    let mut tx = pool.begin().await?;

    let (source_id, target_id, snapshot) =
        snapshot_source_book(&mut tx, source_uuid, target_uuid).await?;

    migrate_book_files(&mut tx, source_id, target_id, &snapshot).await?;
    move_links(&mut tx, source_id, target_id).await?;
    move_progress_and_history(&mut tx, source_id, target_id).await?;
    move_identifiers(&mut tx, source_id, target_id).await?;
    merge_overrides(&mut tx, source_uuid, target_uuid, merged_by).await?;
    let adopt_cover = adopt_cover_flag(&mut tx, source_id, target_id).await?;

    // FTS5 has no FK cascade; clear the source row explicitly via the
    // door. The target's row is rebuilt post-commit with the unioned
    // links.
    delete_fts(&mut tx, source_id).await?;

    rewire_merged_uuid_refs(&mut tx, source_id, target_id, source_uuid, &snapshot).await?;

    let merge_log_id = finalize_merge(&mut tx, target_id, source_id, &snapshot, merged_by).await?;

    tx.commit().await?;

    run_post_commit_side_effects(pool, source_uuid, target_uuid, adopt_cover).await;

    Ok(MergeOutcome {
        merge_log_id,
        target_uuid: target_uuid.to_owned(),
    })
}

/// Re-point earlier merge/attach ledger rows from the source onto the
/// target, then plant a guard row per native format so the next reindex
/// re-attaches the source file by `scan_key` instead of resurrecting the
/// duplicate. Must run before the source `books` row is deleted in
/// `finalize_merge` — that delete cascades through `merged_uuids`.
async fn rewire_merged_uuid_refs(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_id: i64,
    target_id: i64,
    source_uuid: &str,
    snapshot: &SourceSnapshot,
) -> Result<(), sqlx::Error> {
    sqlx::query("UPDATE merged_uuids SET book_id = ? WHERE book_id = ?")
        .bind(target_id)
        .bind(source_id)
        .execute(&mut **tx)
        .await?;
    // The source file's relative path (F2 scan_key) so the next reindex
    // recognizes the now-attached file by scan_key and classifies it
    // Unchanged instead of resurrecting a duplicate.
    let source_scan_key: Option<String> =
        sqlx::query_scalar("SELECT scan_key FROM books WHERE id = ?")
            .bind(source_id)
            .fetch_optional(&mut **tx)
            .await?;
    for fmt in &snapshot.native_formats {
        sqlx::query(
            "INSERT OR REPLACE INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(source_uuid)
        .bind(target_id)
        .bind(fmt)
        .bind(&snapshot.library_path)
        .bind(&source_scan_key)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Best-effort post-commit fixups: adopt the source's cover file under
/// the target's uuid (cover files are uuid-named) and rebuild the
/// target's FTS row so the unioned authors/tags are searchable. Failures
/// are logged but never propagated — the commit has already landed.
async fn run_post_commit_side_effects(
    pool: &SqlitePool,
    source_uuid: &str,
    target_uuid: &str,
    adopt_cover: bool,
) {
    if adopt_cover {
        let src = source_uuid.to_owned();
        let tgt = target_uuid.to_owned();
        let _ = tokio::task::spawn_blocking(move || {
            if let Some((mime, bytes)) = find_cover_file(&src) {
                if let Err(e) = write_cover_file(&tgt, &mime, &bytes) {
                    tracing::warn!(error = %e, uuid = %tgt, "merge: cover adopt copy failed");
                }
            }
        })
        .await;
    }
    if let Err(e) =
        crate::metadata_overrides::rebuild_fts_for_books_batch(pool, &[target_uuid.to_owned()])
            .await
    {
        tracing::warn!(error = %e, uuid = %target_uuid, "merge: target FTS rebuild failed");
    }
}

/// Resolve both uuids to row ids, re-check same-book, then freeze the
/// source book's metadata into a [`SourceSnapshot`] for the merge log.
async fn snapshot_source_book(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_uuid: &str,
    target_uuid: &str,
) -> Result<(i64, i64, SourceSnapshot), MergeError> {
    let source_id = resolve_book(tx, source_uuid).await?;
    let target_id = resolve_book(tx, target_uuid).await?;
    if source_id == target_id {
        return Err(MergeError::SameBook);
    }
    let snapshot = build_snapshot(tx, source_id).await?;
    Ok((source_id, target_id, snapshot))
}

/// Re-parent the source's `book_files` onto the target and renormalize
/// the moved rows' ordinals/labels.
async fn migrate_book_files(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_id: i64,
    target_id: i64,
    snapshot: &SourceSnapshot,
) -> Result<(), sqlx::Error> {
    move_book_files(
        tx,
        source_id,
        target_id,
        &snapshot.library_path,
        &snapshot.path,
    )
    .await?;
    assign_ordinals_after_move(tx, target_id, &snapshot.title).await?;
    Ok(())
}

/// Append the merge-log entry and delete the source book row. Returns
/// the new `merge_log.id` (the undo handle).
async fn finalize_merge(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    target_id: i64,
    source_id: i64,
    snapshot: &SourceSnapshot,
    merged_by: Option<i64>,
) -> Result<i64, MergeError> {
    let merge_log_id: i64 = sqlx::query_scalar(
        "INSERT INTO merge_log (target_book_id, source_uuid, source_metadata, merged_by)
         VALUES (?, ?, ?, ?) RETURNING id",
    )
    .bind(target_id)
    .bind(&snapshot.uuid)
    .bind(serde_json::to_string(snapshot)?)
    .bind(merged_by)
    .fetch_one(&mut **tx)
    .await?;

    sqlx::query("DELETE FROM books WHERE id = ?")
        .bind(source_id)
        .execute(&mut **tx)
        .await?;

    Ok(merge_log_id)
}

/// Resolve a uuid to its `books.id` — real rows only, no `merged_uuids`
/// fallback (you can't merge a book that no longer exists).
async fn resolve_book(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    uuid: &str,
) -> Result<i64, MergeError> {
    sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE uuid = ?")
        .bind(uuid)
        .fetch_optional(&mut **tx)
        .await?
        .ok_or_else(|| MergeError::BookNotFound(uuid.to_owned()))
}

/// Sentinel offset added by `move_book_files` to source ordinals before
/// the re-parent, preventing UNIQUE constraint violations during the move.
const ORDINAL_OFFSET: i64 = 1000;

/// After re-parenting source files onto the target, assign clean ordinals
/// to moved files. Moved files carry temporary ordinals (>= ORDINAL_OFFSET)
/// from `move_book_files`; this normalizes them to consecutive values
/// starting after the target's existing files. Their `label` is set to
/// the source book's title so the file picker shows a meaningful name.
async fn assign_ordinals_after_move(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    target_id: i64,
    source_title: &str,
) -> Result<(), sqlx::Error> {
    let formats_with_moves: Vec<String> = sqlx::query_scalar(
        "SELECT DISTINCT format FROM book_files
          WHERE book_id = ? AND ordinal >= ?",
    )
    .bind(target_id)
    .bind(ORDINAL_OFFSET)
    .fetch_all(&mut **tx)
    .await?;

    for fmt in &formats_with_moves {
        let max_existing: i64 = sqlx::query_scalar(
            "SELECT COALESCE(MAX(ordinal), -1) FROM book_files
              WHERE book_id = ? AND format = ? AND ordinal < ?",
        )
        .bind(target_id)
        .bind(fmt)
        .bind(ORDINAL_OFFSET)
        .fetch_one(&mut **tx)
        .await?;

        let moved_ids: Vec<i64> = sqlx::query_scalar(
            "SELECT id FROM book_files
              WHERE book_id = ? AND format = ? AND ordinal >= ?
              ORDER BY ordinal, filename",
        )
        .bind(target_id)
        .bind(fmt)
        .bind(ORDINAL_OFFSET)
        .fetch_all(&mut **tx)
        .await?;

        for (i, file_id) in moved_ids.iter().enumerate() {
            let new_ordinal = max_existing + 1 + i as i64;
            sqlx::query(
                "UPDATE book_files SET ordinal = ?, label = COALESCE(label, ?)
                  WHERE id = ?",
            )
            .bind(new_ordinal)
            .bind(source_title)
            .bind(file_id)
            .execute(&mut **tx)
            .await?;
        }
    }
    Ok(())
}

/// Re-parent the source's `book_files` rows (parts and chapters follow
/// via their `book_file_id` FK). To avoid violating the
/// `UNIQUE(book_id, format, ordinal)` constraint, source files are given
/// temporary high ordinals before the re-parent; `assign_ordinals_after_move`
/// cleans them up to consecutive values.
async fn move_book_files(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_id: i64,
    target_id: i64,
    source_library_path: &str,
    source_path: &str,
) -> Result<(), sqlx::Error> {
    let max_target: i64 =
        sqlx::query_scalar("SELECT COALESCE(MAX(ordinal), -1) FROM book_files WHERE book_id = ?")
            .bind(target_id)
            .fetch_one(&mut **tx)
            .await?;

    // Offset source ordinals so they land above anything on the target,
    // preventing collisions during the re-parent UPDATE.
    sqlx::query("UPDATE book_files SET ordinal = ordinal + ?1 WHERE book_id = ?2")
        .bind(max_target + ORDINAL_OFFSET)
        .bind(source_id)
        .execute(&mut **tx)
        .await?;

    sqlx::query(
        "UPDATE book_files SET
            book_id = ?1,
            library_path = COALESCE(library_path, ?3),
            path = COALESCE(path, ?4)
         WHERE book_id = ?2",
    )
    .bind(target_id)
    .bind(source_id)
    .bind(source_library_path)
    .bind(source_path)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Additive union of all five m2m link tables, then clear the source's
/// rows (the cascade would do it, but being explicit keeps the
/// transaction's effects readable).
async fn move_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_id: i64,
    target_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO books_authors_link (book, author, position)
         SELECT ?1, author, position FROM books_authors_link WHERE book = ?2",
    )
    .bind(target_id)
    .bind(source_id)
    .execute(&mut **tx)
    .await?;
    for (table, col) in [
        ("books_series_link", "series"),
        ("books_tags_link", "tag"),
        ("books_publishers_link", "publisher"),
        ("books_languages_link", "language"),
    ] {
        let sql = format!(
            "INSERT OR IGNORE INTO {table} (book, {col})
             SELECT ?1, {col} FROM {table} WHERE book = ?2"
        );
        sqlx::query(&sql)
            .bind(target_id)
            .bind(source_id)
            .execute(&mut **tx)
            .await?;
    }
    for table in [
        "books_authors_link",
        "books_series_link",
        "books_tags_link",
        "books_publishers_link",
        "books_languages_link",
    ] {
        let sql = format!("DELETE FROM {table} WHERE book = ?");
        sqlx::query(&sql).bind(source_id).execute(&mut **tx).await?;
    }
    Ok(())
}

/// Re-parent reading progress (with latest-wins dedupe) plus bookmarks,
/// reading/listening sessions, and highlights from the source book onto the
/// target. The F1 user-data tables soft-reference the durable `books.uuid`,
/// so re-parenting is an `UPDATE … SET book_uuid = <target> WHERE book_uuid =
/// <source>` (no FK cascade — a cascade would *delete* the children). The two
/// canonical uuids are read from `books` while the source row still exists
/// (it is deleted later in `finalize_merge`).
///
/// Dedupe on `reading_progress` is necessary because `format` is the coarse
/// `'epub' | 'audio'`, while the merge's format-collision check uses the real
/// file formats — an M4B book merging with an MP3 book passes the check but
/// both carry `'audio'` progress rows.
async fn move_progress_and_history(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_id: i64,
    target_id: i64,
) -> Result<(), sqlx::Error> {
    let source_uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(source_id)
        .fetch_one(&mut **tx)
        .await?;
    let target_uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
        .bind(target_id)
        .fetch_one(&mut **tx)
        .await?;

    // Losing source rows first (target is newer or equal) …
    sqlx::query(
        "DELETE FROM reading_progress WHERE book_uuid = ?2 AND EXISTS (
            SELECT 1 FROM reading_progress t
             WHERE t.book_uuid = ?1 AND t.user_id = reading_progress.user_id
               AND t.format = reading_progress.format
               AND t.updated_at >= reading_progress.updated_at)",
    )
    .bind(&target_uuid)
    .bind(&source_uuid)
    .execute(&mut **tx)
    .await?;
    // … then losing target rows (a strictly newer source row survived).
    sqlx::query(
        "DELETE FROM reading_progress WHERE book_uuid = ?1 AND EXISTS (
            SELECT 1 FROM reading_progress s
             WHERE s.book_uuid = ?2 AND s.user_id = reading_progress.user_id
               AND s.format = reading_progress.format)",
    )
    .bind(&target_uuid)
    .bind(&source_uuid)
    .execute(&mut **tx)
    .await?;
    for table in [
        "reading_progress",
        "bookmarks",
        "reading_sessions",
        "listening_sessions",
        "highlights",
    ] {
        let sql = format!("UPDATE {table} SET book_uuid = ?1 WHERE book_uuid = ?2");
        sqlx::query(&sql)
            .bind(&target_uuid)
            .bind(&source_uuid)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Union identifiers; the target's value wins per `(book_id, scheme)`.
/// The source's own rows cascade with the source delete.
async fn move_identifiers(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_id: i64,
    target_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "INSERT OR IGNORE INTO book_identifiers (book_id, scheme, value)
         SELECT ?1, scheme, value FROM book_identifiers WHERE book_id = ?2",
    )
    .bind(target_id)
    .bind(source_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// Shallow-merge the source's `metadata_overrides` into the target's
/// (target keys win) and drop the source row. The target's
/// `has_cover_override` flag is preserved; the source's uploaded
/// override cover (if any) is not adopted — a documented simplification.
async fn merge_overrides(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_uuid: &str,
    target_uuid: &str,
    merged_by: Option<i64>,
) -> Result<(), MergeError> {
    let source_row: Option<(String,)> =
        sqlx::query_as("SELECT overrides FROM metadata_overrides WHERE book_uuid = ?")
            .bind(source_uuid)
            .fetch_optional(&mut **tx)
            .await?;
    let Some((source_json,)) = source_row else {
        return Ok(());
    };
    let source_ov: MetadataOverrides = serde_json::from_str(&source_json)?;

    let target_row: Option<(String, i64)> = sqlx::query_as(
        "SELECT overrides, has_cover_override FROM metadata_overrides WHERE book_uuid = ?",
    )
    .bind(target_uuid)
    .fetch_optional(&mut **tx)
    .await?;
    let (target_ov, target_flag) = match target_row {
        Some((json, flag)) => (serde_json::from_str(&json)?, flag != 0),
        None => (MetadataOverrides::default(), false),
    };

    // `merge` is incoming-wins, so incoming = target keeps target keys.
    let merged = source_ov.merge(&target_ov);
    sqlx::query(
        "INSERT INTO metadata_overrides
            (book_uuid, overrides, has_cover_override, updated_by, updated_at)
         VALUES (?, ?, ?, ?, datetime('now'))
         ON CONFLICT(book_uuid) DO UPDATE SET
           overrides = excluded.overrides,
           has_cover_override = excluded.has_cover_override,
           updated_by = excluded.updated_by,
           updated_at = datetime('now')",
    )
    .bind(target_uuid)
    .bind(serde_json::to_string(&merged)?)
    .bind(i64::from(target_flag))
    .bind(merged_by)
    .execute(&mut **tx)
    .await?;
    sqlx::query("DELETE FROM metadata_overrides WHERE book_uuid = ?")
        .bind(source_uuid)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Cover precedence: target wins; adopt the source's scanned cover only
/// when the target has none. Returns whether the post-commit file copy
/// should run.
async fn adopt_cover_flag(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    source_id: i64,
    target_id: i64,
) -> Result<bool, sqlx::Error> {
    let (source_has, target_has): (i64, i64) = sqlx::query_as(
        "SELECT (SELECT has_cover FROM books WHERE id = ?1),
                (SELECT has_cover FROM books WHERE id = ?2)",
    )
    .bind(source_id)
    .bind(target_id)
    .fetch_one(&mut **tx)
    .await?;
    if source_has != 0 && target_has == 0 {
        sqlx::query("UPDATE books SET has_cover = 1 WHERE id = ?")
            .bind(target_id)
            .execute(&mut **tx)
            .await?;
        return Ok(true);
    }
    Ok(false)
}
