//! Multi-file audiobook sync. Mirrors `sync_books` but writes
//! `book_file_parts` rows in addition to `books` and `book_files`. Like
//! the ebook path, all `books_fts` maintenance routes through the
//! [`super::fts`] choke-point (`upsert_fts` / `delete_fts`).

use std::collections::HashMap;

use sqlx::{SqlitePool, Transaction};

use crate::covers::delete_cover_files_for;
use crate::helpers::{mint_uuid, sanitize_accent_color, stable_uuid};
use crate::normalize::{normalize_author, normalize_title};
use crate::settings::upsert_library;

use super::attach;
use super::books::{materialize_new_covers, SyncError};
use super::fts::upsert_fts;

/// Per-bucket payload for [`sync_audiobooks`]. Mirrors [`SyncPlan`] for
/// the ebook path but carries [`crate::audiobook::IndexedAudiobook`] rows
/// that include the ordered `book_file_parts` list.
#[derive(Debug, Default)]
pub struct AudiobookSyncPlan {
    pub new_books: Vec<crate::audiobook::IndexedAudiobook>,
    pub changed_books: Vec<crate::audiobook::IndexedAudiobook>,
    pub removed_uuids: Vec<String>,
    /// `(uuid, mtime_epoch, size_bytes)` stat-only backfill (no re-parse).
    pub backfill: Vec<(String, i64, i64)>,
}

/// Apply a multi-file audiobook sync plan atomically. Mirrors [`sync_books`]
/// but writes `book_file_parts` rows in addition to `books` and `book_files`.
///
/// Transaction order:
/// 1. Upsert `scan_roots` row.
/// 2. Mark Removed files missing (drop `book_files`, retain the `books` row).
/// 3. Update Changed in-place: wipe `book_files` + `book_file_parts` + author
///    link + FTS, then re-insert them.
/// 4. Insert New.
/// 5. Backfill `book_files.(mtime_epoch, size_bytes)` only.
/// 6. Stamp `scan_roots.last_indexed`.
///
/// Post-commit: write / delete cover files (best-effort, same as sync_books).
pub async fn sync_audiobooks(
    pool: &SqlitePool,
    library_path: &str,
    plan: AudiobookSyncPlan,
) -> anyhow::Result<()> {
    sync_audiobooks_with_progress(pool, library_path, plan, |_, _| {}).await
}

/// [`sync_audiobooks`] variant that calls `on_progress(processed, total)`
/// after each per-book write. `total` counts the buckets that loop per
/// book — Changed + New. Removed and Backfill are batched and not
/// reported as per-book ticks.
pub async fn sync_audiobooks_with_progress(
    pool: &SqlitePool,
    library_path: &str,
    plan: AudiobookSyncPlan,
    mut on_progress: impl FnMut(u32, u32),
) -> anyhow::Result<()> {
    let total: u32 = (plan.changed_books.len() + plan.new_books.len())
        .try_into()
        .unwrap_or(u32::MAX);
    // Emit (0, total) before any per-book work so the UI flips from
    // indeterminate spinner to determinate bar on the first poll.
    on_progress(0, total);
    let mut processed: u32 = 0;

    let mut tx = pool.begin().await?;
    let library_id = upsert_library(&mut tx, library_path).await?;

    sync_audiobooks_removed(&mut tx, library_id, &plan.removed_uuids).await?;
    let changed_covers = sync_audiobooks_changed(
        &mut tx,
        library_id,
        library_path,
        &plan.changed_books,
        || {
            processed = processed.saturating_add(1);
            on_progress(processed, total);
        },
    )
    .await?;
    let new_covers =
        sync_audiobooks_new(&mut tx, library_id, library_path, &plan.new_books, || {
            processed = processed.saturating_add(1);
            on_progress(processed, total);
        })
        .await?;
    backfill_audiobook_stats(&mut tx, library_id, &plan.backfill).await?;
    stamp_audiobooks_last_indexed(&mut tx, library_id).await?;

    tx.commit().await?;

    let removed_uuids = plan.removed_uuids;
    if let Err(join_err) = tokio::task::spawn_blocking(move || {
        delete_cover_files_for(&removed_uuids);
        materialize_new_covers(new_covers);
        materialize_new_covers(changed_covers);
    })
    .await
    {
        tracing::error!("sync_audiobooks: cover reconcile spawn_blocking failed: {join_err}");
    }

    Ok(())
}

/// Apply the Removed bucket (F2): resolve affected ids and, via
/// `books::mark_book_files_missing`, drop the `book_files` rows (parts/chapters
/// cascade) and flag each book missing — but **retain** the `books` row, its
/// links, FTS, and soft-ref user data, so the book stays in browse/search (the
/// grid hides it via `EXISTS book_files`) and a returning group re-attaches via
/// Changed. Also drop any `book_files` rows whose uuid lived only in
/// `merged_uuids` (cross-format attachments — the target book survives).
async fn sync_audiobooks_removed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    removed_uuids: &[String],
) -> Result<(), SyncError> {
    if removed_uuids.is_empty() {
        return Ok(());
    }
    let mut missing = 0usize;
    // Chunk at 500 to stay under SQLite's 999-param cap when a whole library
    // (or any large diff) is removed — same convention as `sync_removed` in
    // `books/removed.rs`.
    for chunk in removed_uuids.chunks(500) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        // Resolve affected ids once, then drop their file rows and flag them
        // missing with a single batched DELETE + UPDATE keyed on the id list —
        // retaining each `books` row (and its links/FTS) as a fileless book (F2).
        let id_sql =
            format!("SELECT id FROM books WHERE library_id = ? AND uuid IN ({placeholders})");
        let mut q = sqlx::query_scalar::<_, i64>(&id_sql).bind(library_id);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        let ids = q.fetch_all(&mut **tx).await?;
        if ids.is_empty() {
            continue;
        }
        missing += ids.len();
        mark_book_files_missing_batch(tx, &ids).await?;
    }
    if missing > 0 {
        tracing::info!(
            missing,
            "sync: retained removed audiobooks as fileless books"
        );
    }

    // Removed uuids that were cross-format attachments have no
    // `books` row — drop their `book_files` row + `merged_uuids`
    // entry instead (the target book survives, possibly fileless).
    // `remove_attached_files` already chunks internally.
    attach::remove_attached_files(tx, removed_uuids).await?;
    Ok(())
}

/// Batched form of `books::mark_book_files_missing` for the Removed bucket: one
/// IN-list DELETE of the `book_files` rows (parts/chapters cascade) and one
/// guarded UPDATE flagging the now-fileless `books` rows missing (F2), instead
/// of two statements per book. The UPDATE keeps `mark_book_files_missing`'s
/// guards — `is_missing_files = 0` preserves the original `missing_files_since`
/// on a re-run, and `is_missing_files_override = 0` leaves intentionally-fileless
/// rows (wishlist) un-flagged. `ids` must be non-empty and within the SQLite
/// 999-param cap (the caller chunks at 500).
async fn mark_book_files_missing_batch(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    ids: &[i64],
) -> Result<(), SyncError> {
    let placeholders = std::iter::repeat_n("?", ids.len())
        .collect::<Vec<_>>()
        .join(", ");

    let del_sql = format!("DELETE FROM book_files WHERE book_id IN ({placeholders})");
    let mut del_q = sqlx::query(&del_sql);
    for id in ids {
        del_q = del_q.bind(id);
    }
    del_q.execute(&mut **tx).await?;

    let upd_sql = format!(
        "UPDATE books
            SET is_missing_files = 1, missing_files_since = unixepoch()
          WHERE id IN ({placeholders}) AND is_missing_files = 0 AND is_missing_files_override = 0"
    );
    let mut upd_q = sqlx::query(&upd_sql);
    for id in ids {
        upd_q = upd_q.bind(id);
    }
    upd_q.execute(&mut **tx).await?;
    Ok(())
}

/// Apply the Changed bucket: batch-resolve uuid → book_id, then per book
/// either refresh the attached file row (uuid lives in `merged_uuids`),
/// promote to a New insert (TOCTOU — diff said the uuid existed but a
/// concurrent process removed it), or wipe-and-rewrite the file/parts/
/// author rows in place. Returns `(uuid, mime, bytes)` triples for the
/// post-commit cover materialization.
async fn sync_audiobooks_changed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    library_path: &str,
    changed_books: &[crate::audiobook::IndexedAudiobook],
    mut on_book_written: impl FnMut(),
) -> Result<Vec<(String, String, Vec<u8>)>, SyncError> {
    let mut changed_covers: Vec<(String, String, Vec<u8>)> = Vec::new();
    if changed_books.is_empty() {
        return Ok(changed_covers);
    }
    // Pre-fetch all book ids in one batch query (chunked at 499 to stay
    // under SQLite's 999-parameter cap), keyed on the F2 `scan_key` (the
    // group's relative path); carry each row's durable `uuid` back for the
    // cover triple.
    let all_scan_keys: Vec<String> = changed_books.iter().map(|b| b.scan_key.clone()).collect();
    let mut id_map: HashMap<String, (i64, String)> = HashMap::new();
    for chunk in all_scan_keys.chunks(499) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let id_sql = format!(
            "SELECT scan_key, id, uuid FROM books
              WHERE library_id = ? AND scan_key IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (String, i64, String)>(&id_sql).bind(library_id);
        for sk in chunk {
            q = q.bind(sk);
        }
        for (sk, id, uuid) in q.fetch_all(&mut **tx).await? {
            id_map.insert(sk, (id, uuid));
        }
    }

    for b in changed_books {
        let Some((book_id, uuid)) = id_map.get(&b.scan_key).map(|(id, u)| (*id, u.clone())) else {
            // No primary books row with this scan_key — either an attachment
            // on another book (matched by the repoint-stable
            // `(library_path, scan_key)`) or a TOCTOU promote to a New insert.
            // (A fileless book whose group returned still has its books row,
            // so it takes the update branch below and re-creates book_files.)
            if let Some((_uuid, target_id, format)) =
                attach::find_attachment_by_scan_key(tx, library_path, &b.scan_key).await?
            {
                attach_audiobook_file(tx, target_id, &format, library_path, b, &mut changed_covers)
                    .await?;
                on_book_written();
                continue;
            }
            insert_new_audiobook(tx, library_id, b, &mut changed_covers).await?;
            on_book_written();
            continue;
        };

        rewrite_audiobook_in_place(tx, book_id, &uuid, b, &mut changed_covers).await?;
        on_book_written();
    }
    Ok(changed_covers)
}

/// Rewrite an existing audiobook in place from a freshly-parsed group,
/// preserving `books.id`/`books.uuid`: refresh the `books` scalars, wipe +
/// re-insert this format's `book_files`/parts/chapters and the author link,
/// and refresh FTS. Shared by the Changed update path and the New re-attach
/// path (a fileless book whose group returned, or a `replace_books`
/// re-add). The `book_files` delete is scoped to this group's own format so
/// a cross-format attachment (e.g. an EPUB row via `merged_uuids`) survives.
async fn rewrite_audiobook_in_place(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    uuid: &str,
    b: &crate::audiobook::IndexedAudiobook,
    covers: &mut Vec<(String, String, Vec<u8>)>,
) -> Result<(), SyncError> {
    update_audiobook_row(tx, book_id, b).await?;
    sqlx::query("DELETE FROM books_authors_link WHERE book = ?")
        .bind(book_id)
        .execute(&mut **tx)
        .await?;
    sqlx::query("DELETE FROM book_files WHERE book_id = ? AND format = ?")
        .bind(book_id)
        .bind(&b.format)
        .execute(&mut **tx)
        .await?;
    let book_file_id = insert_audiobook_file_row(tx, book_id, b).await?;
    insert_audiobook_parts(tx, book_file_id, &b.parts).await?;
    insert_chapters(tx, book_file_id, &b.chapters, &b.parts).await?;
    insert_audiobook_author_link(tx, book_id, b.creator_name.as_deref()).await?;
    upsert_fts(tx, book_id).await?;
    if let Some((mime, bytes)) = &b.cover {
        covers.push((uuid.to_string(), mime.clone(), bytes.clone()));
    }
    Ok(())
}

/// Apply the New bucket: for each entry try cross-format attach first,
/// otherwise insert a fresh `books` + `book_files` + parts + chapters +
/// author-link + FTS row. Returns the post-commit cover triples.
async fn sync_audiobooks_new(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    library_path: &str,
    new_books: &[crate::audiobook::IndexedAudiobook],
    mut on_book_written: impl FnMut(),
) -> Result<Vec<(String, String, Vec<u8>)>, SyncError> {
    let mut new_covers: Vec<(String, String, Vec<u8>)> = Vec::new();
    if new_books.is_empty() {
        return Ok(new_covers);
    }
    // Pre-fetch every same-scan_key `books` row in one batch (chunked at 499 to
    // stay under SQLite's 999-param cap), keyed on the F2 `scan_key` — mirrors
    // `sync_audiobooks_changed`. New entries carry distinct scan_keys, so each
    // maps at most one existing row and the map never goes stale mid-loop.
    let all_scan_keys: Vec<String> = new_books.iter().map(|b| b.scan_key.clone()).collect();
    let mut id_map: HashMap<String, (i64, String)> = HashMap::new();
    for chunk in all_scan_keys.chunks(499) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let id_sql = format!(
            "SELECT scan_key, id, uuid FROM books
              WHERE library_id = ? AND scan_key IN ({placeholders})"
        );
        let mut q = sqlx::query_as::<_, (String, i64, String)>(&id_sql).bind(library_id);
        for sk in chunk {
            q = q.bind(sk);
        }
        for (sk, id, uuid) in q.fetch_all(&mut **tx).await? {
            id_map.insert(sk, (id, uuid));
        }
    }

    for b in new_books {
        // Same-scan_key row (a fileless book whose group returned, or a
        // `replace_books` re-add) → rewrite in place, *before* the
        // cross-format attach heuristic, preserving `books.uuid`.
        if let Some((book_id, uuid)) = id_map.get(&b.scan_key).map(|(id, u)| (*id, u.clone())) {
            rewrite_audiobook_in_place(tx, book_id, &uuid, b, &mut new_covers).await?;
            on_book_written();
            continue;
        }
        if try_attach_new_audiobook(tx, library_path, b, &mut new_covers).await? {
            on_book_written();
            continue;
        }
        insert_new_audiobook(tx, library_id, b, &mut new_covers).await?;
        on_book_written();
    }
    Ok(new_covers)
}

/// Insert a fresh audiobook (canonical `books` + `book_files` + parts +
/// chapters + author-link + FTS row) and push its cover triple if any.
/// Shared by `sync_audiobooks_new` and the TOCTOU promote inside
/// `sync_audiobooks_changed`.
async fn insert_new_audiobook(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    b: &crate::audiobook::IndexedAudiobook,
    covers: &mut Vec<(String, String, Vec<u8>)>,
) -> Result<(), SyncError> {
    let inserted = insert_audiobook_row(tx, library_id, b).await?;
    insert_audiobook_parts(tx, inserted.book_file_id, &b.parts).await?;
    insert_chapters(tx, inserted.book_file_id, &b.chapters, &b.parts).await?;
    insert_audiobook_author_link(tx, inserted.book_id, b.creator_name.as_deref()).await?;
    upsert_fts(tx, inserted.book_id).await?;
    if let Some((mime, bytes)) = &b.cover {
        covers.push((inserted.uuid, mime.clone(), bytes.clone()));
    }
    Ok(())
}

/// Apply the stat-only backfill: UPDATE `book_files.(mtime_epoch, size_bytes)`
/// in chunks of 250 (3 binds per row + library_id keeps us under SQLite's
/// 999-parameter cap). No OPF re-parse, no link writes, no FTS write.
async fn backfill_audiobook_stats(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    backfill: &[(String, i64, i64)],
) -> Result<(), SyncError> {
    for chunk in backfill.chunks(250) {
        let rows = std::iter::repeat_n("(?, ?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "UPDATE book_files SET mtime_epoch = v.column2, size_bytes = v.column3 \
             FROM (VALUES {rows}) AS v, books b \
             WHERE b.uuid = v.column1 AND b.library_id = ? AND book_files.book_id = b.id"
        );
        let mut q = sqlx::query(&sql);
        for (uuid, mtime_epoch, size_bytes) in chunk {
            q = q.bind(uuid).bind(mtime_epoch).bind(size_bytes);
        }
        q = q.bind(library_id);
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Stamp `scan_roots.last_indexed` with the current unix epoch — the last
/// step inside the sync transaction.
async fn stamp_audiobooks_last_indexed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
) -> Result<(), SyncError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    sqlx::query("UPDATE scan_roots SET last_indexed = ? WHERE id = ?")
        .bind(now)
        .bind(library_id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

/// Try to attach a brand-new audiobook group to an existing book in
/// another format instead of inserting a fresh `books` row. Mirrors
/// `books::try_attach_new_ebook`: a `merged_uuids` hit attaches
/// unconditionally; otherwise exactly one normalized title+author match
/// without this format attaches. Returns `true` when attached.
async fn try_attach_new_audiobook(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_path: &str,
    b: &crate::audiobook::IndexedAudiobook,
    covers: &mut Vec<(String, String, Vec<u8>)>,
) -> Result<bool, SyncError> {
    if b.error.is_some() {
        return Ok(false);
    }
    // Already a recorded attachment? Match by the repoint-stable relative
    // `scan_key` (the group path) and re-attach against the stored ledger uuid.
    if let Some((_uuid, target_id, format)) =
        attach::find_attachment_by_scan_key(tx, library_path, &b.scan_key).await?
    {
        attach_audiobook_file(tx, target_id, &format, library_path, b, covers).await?;
        return Ok(true);
    }
    let (Some(title_norm), Some(author_norm)) = (
        normalize_title(&b.title),
        b.creator_name.as_deref().and_then(normalize_author),
    ) else {
        // No author (or empty title): too weak a signal to auto-match.
        return Ok(false);
    };
    let Some(target_id) =
        attach::find_attach_target(tx, &title_norm, &author_norm, &b.format).await?
    else {
        return Ok(false);
    };
    attach_audiobook_file(tx, target_id, &b.format, library_path, b, covers).await?;
    Ok(true)
}

/// Write (or rewrite) an attached audiobook's `book_files` row (plus
/// parts and chapters) under `book_id`, record the attachment, and adopt
/// the cover when the target has none. The target's `books` scalars and
/// links are deliberately left untouched, but its FTS row is refreshed
/// via the door so any newly-unioned text becomes searchable. The file
/// row carries its own `(library_path, path)` location override — the
/// target book may live in a different library, and the HLS read path
/// resolves part filenames against the *audio* root.
async fn attach_audiobook_file(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    format: &str,
    library_path: &str,
    b: &crate::audiobook::IndexedAudiobook,
    covers: &mut Vec<(String, String, Vec<u8>)>,
) -> Result<(), SyncError> {
    // Idempotent re-attach: the refresh path lands here too.
    sqlx::query("DELETE FROM book_files WHERE book_id = ? AND format = ?")
        .bind(book_id)
        .bind(format)
        .execute(&mut **tx)
        .await?;
    let book_file_id = insert_audiobook_file_row(tx, book_id, b).await?;
    let dir = std::path::Path::new(&b.group_path)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("")
        .to_string();
    sqlx::query("UPDATE book_files SET library_path = ?, path = ? WHERE id = ?")
        .bind(library_path)
        .bind(&dir)
        .bind(book_file_id)
        .execute(&mut **tx)
        .await?;
    insert_audiobook_parts(tx, book_file_id, &b.parts).await?;
    insert_chapters(tx, book_file_id, &b.chapters, &b.parts).await?;
    // Merged-ledger key = stable_uuid(group path) (unchanged); scan_key =
    // the group's relative path, the F2 diff key for repoint survival.
    let merged_key = stable_uuid(library_path, &b.group_path);
    attach::record_attachment(tx, &merged_key, book_id, format, library_path, &b.scan_key).await?;
    // Refresh the target's FTS row so this format's contribution to the
    // searchable text lands immediately (mirrors the ebook attach path).
    upsert_fts(tx, book_id).await?;
    if let Some(cover) = attach::maybe_adopt_cover(tx, book_id, b.cover.as_ref()).await? {
        covers.push(cover);
    }
    Ok(())
}

/// Return type from a fresh audiobook insert — the ids needed by the
/// caller for the parts + FTS + author-link inserts, plus the minted
/// `uuid` for the post-commit cover triple.
struct InsertedAudiobook {
    book_id: i64,
    book_file_id: i64,
    uuid: String,
}

/// Insert the `books` row and its single `book_files` row for a new
/// audiobook, returning the ids of both and the minted uuid.
async fn insert_audiobook_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    b: &crate::audiobook::IndexedAudiobook,
) -> Result<InsertedAudiobook, sqlx::Error> {
    // `path` = parent directory of group_path (the "book folder").
    let book_path = std::path::Path::new(&b.group_path)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("")
        .to_string();
    let has_cover = i64::from(b.cover.is_some());
    // F2: durable v4 identity minted once; the diff matches on scan_key
    // (the group's relative path).
    let uuid = mint_uuid();

    let book_id = sqlx::query_scalar::<_, i64>(
        // `timestamp`/`last_modified` set explicitly — migration 0038 dropped
        // their column default when converting `books` in place to INTEGER.
        "INSERT INTO books \
            (uuid, scan_key, library_id, path, title, sort, author_sort, has_cover, description, \
             accent_color, title_norm, author_norm, timestamp, last_modified) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, \
                 strftime('%s','now'), strftime('%s','now')) \
         RETURNING id",
    )
    .bind(&uuid)
    .bind(&b.scan_key)
    .bind(library_id)
    .bind(&book_path)
    .bind(&b.title)
    .bind(&b.title)
    .bind(&b.creator_name)
    .bind(has_cover)
    .bind(&b.description)
    .bind(sanitize_accent_color(b.accent.as_deref()))
    .bind(normalize_title(&b.title))
    .bind(b.creator_name.as_deref().and_then(normalize_author))
    .fetch_one(&mut **tx)
    .await?;

    let book_file_id = insert_audiobook_file_row(tx, book_id, b).await?;
    Ok(InsertedAudiobook {
        book_id,
        book_file_id,
        uuid,
    })
}

/// Insert the `book_files` row for an audiobook (used by both New and
/// the re-insert step of Changed). Returns the new `book_file_id`.
async fn insert_audiobook_file_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    b: &crate::audiobook::IndexedAudiobook,
) -> Result<i64, sqlx::Error> {
    // `filename` = leaf stem of group_path (the folder name or file stem).
    let filename = std::path::Path::new(&b.group_path)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or(&b.group_path)
        .to_string();

    let id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO book_files \
            (book_id, format, filename, size_bytes, mtime_epoch) \
         VALUES (?, ?, ?, ?, ?) \
         RETURNING id",
    )
    .bind(book_id)
    .bind(&b.format)
    .bind(&filename)
    .bind(b.total_size_bytes)
    .bind(b.max_mtime_epoch)
    .fetch_one(&mut **tx)
    .await?;
    // A returning group clears the F10 missing-files flag (no-op on a fresh
    // insert) — the audiobook file-write chokepoint, mirroring the ebook path.
    super::books::clear_missing_files_flag(tx, book_id).await?;
    Ok(id)
}

/// Batch-insert `book_file_parts` rows for the ordered part list.
///
/// One VALUES-list `INSERT` per chunk (chunked at 166 rows: 6 binds per row
/// keeps each statement under SQLite's 999 bind-parameter cap) instead of
/// one round-trip per part — the parent transaction's commit boundary is
/// unchanged.
async fn insert_audiobook_parts(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_file_id: i64,
    parts: &[crate::audiobook::AudiobookPart],
) -> Result<(), sqlx::Error> {
    // 6 binds per row × 166 rows = 996 binds, under SQLite's 999 cap.
    for chunk in parts.chunks(166) {
        let rows = std::iter::repeat_n("(?, ?, ?, ?, ?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT INTO book_file_parts \
                (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
             VALUES {rows}"
        );
        let mut q = sqlx::query(&sql);
        for p in chunk {
            q = q
                .bind(book_file_id)
                .bind(p.ordinal)
                .bind(&p.filename)
                .bind(p.size_bytes)
                .bind(p.mtime_epoch)
                .bind(p.duration_seconds);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Insert `file_chapters` rows. If no chapters were extracted, synthesize
/// one chapter per part — yielding zero rows only when both `chapters` and
/// `parts` are empty (the empty-parts edge case).
///
/// Both branches first materialize the row tuples and then hand them to
/// [`bulk_insert_chapters`], which issues a single VALUES-list `INSERT` per
/// chunk (199 rows × 5 binds = 995, under SQLite's 999 bind cap) instead of
/// one round-trip per chapter.
pub(crate) async fn insert_chapters(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_file_id: i64,
    chapters: &[crate::audiobook::RawChapter],
    parts: &[crate::audiobook::AudiobookPart],
) -> Result<(), SyncError> {
    // (ordinal, title, start_seconds, duration_seconds). Exactly one branch
    // below runs, so the needed capacity is whichever input it drains.
    let capacity = if chapters.is_empty() {
        parts.len()
    } else {
        chapters.len()
    };
    let mut rows: Vec<(i64, String, f64, f64)> = Vec::with_capacity(capacity);
    if !chapters.is_empty() {
        let total_duration: f64 = parts.iter().map(|p| p.duration_seconds).sum();
        // Chapter timestamps are milliseconds; `f64` has 53 bits of
        // integer precision, so it represents every millisecond exactly
        // up to ~2^53 ms ≈ 285,000 years — well past any real audiobook.
        #[allow(clippy::cast_precision_loss)]
        for (i, ch) in chapters.iter().enumerate() {
            let start_seconds = ch.start_ms as f64 / 1000.0;
            let duration_seconds = if ch.end_ms > ch.start_ms {
                (ch.end_ms - ch.start_ms) as f64 / 1000.0
            } else if i + 1 < chapters.len() {
                (chapters[i + 1].start_ms - ch.start_ms) as f64 / 1000.0
            } else {
                (total_duration - start_seconds).max(0.0)
            };
            rows.push((i as i64, ch.title.clone(), start_seconds, duration_seconds));
        }
    } else {
        // Synthetic fallback: one chapter per part.
        let mut cumulative = 0.0f64;
        for p in parts {
            let title = format!("Part {}", p.ordinal + 1);
            rows.push((p.ordinal, title, cumulative, p.duration_seconds));
            cumulative += p.duration_seconds;
        }
    }
    bulk_insert_chapters(tx, book_file_id, &rows).await?;
    Ok(())
}

/// Bulk-insert pre-materialized `file_chapters` rows. Chunks at 199 rows so
/// 5 binds per row stays under SQLite's 999 bind-parameter cap.
async fn bulk_insert_chapters(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_file_id: i64,
    rows: &[(i64, String, f64, f64)],
) -> Result<(), sqlx::Error> {
    for chunk in rows.chunks(199) {
        let placeholders = std::iter::repeat_n("(?, ?, ?, ?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT INTO file_chapters \
                (book_file_id, ordinal, title, start_seconds, duration_seconds) \
             VALUES {placeholders}"
        );
        let mut q = sqlx::query(&sql);
        for (ordinal, title, start_seconds, duration_seconds) in chunk {
            q = q
                .bind(book_file_id)
                .bind(*ordinal)
                .bind(title)
                .bind(*start_seconds)
                .bind(*duration_seconds);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Resolve or insert the author and link them to `book_id`. A `None`
/// creator_name is a no-op — the book goes authorless.
async fn insert_audiobook_author_link(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    creator_name: Option<&str>,
) -> Result<(), sqlx::Error> {
    let Some(name) = creator_name.filter(|n| !n.is_empty()) else {
        return Ok(());
    };
    sqlx::query(
        "INSERT INTO authors (name, sort) VALUES (?, ?) \
         ON CONFLICT(name) DO UPDATE SET sort = COALESCE(authors.sort, excluded.sort)",
    )
    .bind(name)
    .bind(name)
    .execute(&mut **tx)
    .await?;

    sqlx::query(
        "INSERT OR IGNORE INTO books_authors_link (book, author, position) \
         SELECT ?, a.id, 0 FROM authors a WHERE a.name = ?",
    )
    .bind(book_id)
    .bind(name)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// UPDATE the `books` row for a Changed audiobook (preserves `books.id`).
async fn update_audiobook_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    b: &crate::audiobook::IndexedAudiobook,
) -> Result<(), sqlx::Error> {
    let book_path = std::path::Path::new(&b.group_path)
        .parent()
        .and_then(|p| p.to_str())
        .unwrap_or("")
        .to_string();
    let has_cover = i64::from(b.cover.is_some());

    sqlx::query(
        "UPDATE books SET \
            path = ?, title = ?, sort = ?, author_sort = ?, has_cover = ?, \
            description = ?, accent_color = ?, title_norm = ?, author_norm = ?, \
            last_modified = strftime('%s','now') \
         WHERE id = ?",
    )
    .bind(&book_path)
    .bind(&b.title)
    .bind(&b.title)
    .bind(&b.creator_name)
    .bind(has_cover)
    .bind(&b.description)
    .bind(sanitize_accent_color(b.accent.as_deref()))
    .bind(normalize_title(&b.title))
    .bind(b.creator_name.as_deref().and_then(normalize_author))
    .bind(book_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
