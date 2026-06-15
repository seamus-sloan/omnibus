//! Multi-file audiobook sync. Mirrors `sync_books` but writes
//! `book_file_parts` rows in addition to `books` and `book_files`. Like
//! the ebook path, all `books_fts` maintenance routes through the
//! [`super::fts`] choke-point (`upsert_fts` / `delete_fts`).

use std::collections::HashMap;

use sqlx::{SqlitePool, Transaction};

use crate::covers::delete_cover_files_for;
use crate::helpers::sanitize_accent_color;
use crate::normalize::{normalize_author, normalize_title};
use crate::settings::upsert_library;

use super::attach;
use super::books::{materialize_new_covers, SyncError};
use super::fts::{delete_fts, upsert_fts};

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
/// 2. Delete Removed (explicit FTS clear + cascade DELETE from `books`).
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
) -> Result<(), SyncError> {
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
) -> Result<(), SyncError> {
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

/// Apply the Removed bucket: resolve affected ids, clear each `books_fts`
/// row through the [`delete_fts`] door, cascade DELETE from `books`, and
/// also drop any `book_files` rows whose uuid lived only in
/// `merged_uuids` (cross-format attachments — the target book survives).
async fn sync_audiobooks_removed(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    removed_uuids: &[String],
) -> Result<(), SyncError> {
    if removed_uuids.is_empty() {
        return Ok(());
    }
    // library_id + 1 bind per uuid; chunk at 499 to stay under SQLite's
    // 999-param cap when a whole library (or any large diff) is removed.
    // Both the id resolve and the `books` delete run per chunk inside the
    // same transaction, mirroring `sync_removed` in `books.rs`.
    for chunk in removed_uuids.chunks(499) {
        let placeholders = std::iter::repeat_n("?", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        // Resolve affected ids, then clear each FTS row via the door
        // before the cascade DELETE on `books` (standalone FTS5, no FK).
        let id_sql =
            format!("SELECT id FROM books WHERE library_id = ? AND uuid IN ({placeholders})");
        let mut q = sqlx::query_scalar::<_, i64>(&id_sql).bind(library_id);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        let ids = q.fetch_all(&mut **tx).await?;
        for id in ids {
            delete_fts(tx, id).await?;
        }

        let books_sql =
            format!("DELETE FROM books WHERE library_id = ? AND uuid IN ({placeholders})");
        let mut q = sqlx::query(&books_sql).bind(library_id);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        q.execute(&mut **tx).await?;
    }

    // Removed uuids that were cross-format attachments have no
    // `books` row — drop their `book_files` row + `merged_uuids`
    // entry instead (the target book survives, possibly fileless).
    // `remove_attached_files` already chunks internally.
    attach::remove_attached_files(tx, removed_uuids).await?;
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
    // under SQLite's 999-parameter cap). Audiobook UUIDs come from
    // `b.uuid` directly — no stable_uuid call needed.
    let all_uuids: Vec<String> = changed_books.iter().map(|b| b.uuid.clone()).collect();
    let mut id_map: HashMap<String, i64> = HashMap::new();
    for chunk in all_uuids.chunks(499) {
        let placeholders = chunk.iter().map(|_| "?").collect::<Vec<_>>().join(", ");
        let id_sql =
            format!("SELECT uuid, id FROM books WHERE library_id = ? AND uuid IN ({placeholders})");
        let mut q = sqlx::query_as::<_, (String, i64)>(&id_sql).bind(library_id);
        for uuid in chunk {
            q = q.bind(uuid);
        }
        id_map.extend(q.fetch_all(&mut **tx).await?);
    }

    for b in changed_books {
        let Some(&book_id) = id_map.get(&b.uuid) else {
            // No books row with this uuid — either an attachment on
            // another book (refresh the file row, leave the target's
            // metadata alone) or a TOCTOU promote to New insert.
            if let Some((target_id, format)) = attach::attach_target_by_uuid(tx, &b.uuid).await? {
                attach_audiobook_file(tx, target_id, &format, library_path, b, &mut changed_covers)
                    .await?;
                on_book_written();
                continue;
            }
            insert_new_audiobook(tx, library_id, b, &mut changed_covers).await?;
            on_book_written();
            continue;
        };

        update_audiobook_row(tx, book_id, b).await?;
        // Wipe dependent rows; ON DELETE CASCADE handles book_file_parts when
        // book_files is deleted. The delete is scoped to this group's
        // own format so a cross-format attachment (e.g. an EPUB row
        // hanging off this book via merged_uuids) survives the
        // re-parse.
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
        // Refresh from the rewritten author link via the door.
        upsert_fts(tx, book_id).await?;

        if let Some((mime, bytes)) = &b.cover {
            changed_covers.push((b.uuid.clone(), mime.clone(), bytes.clone()));
        }
        on_book_written();
    }
    Ok(changed_covers)
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
    for b in new_books {
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
        covers.push((b.uuid.clone(), mime.clone(), bytes.clone()));
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
    if let Some((target_id, format)) = attach::attach_target_by_uuid(tx, &b.uuid).await? {
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
    attach::record_attachment(tx, &b.uuid, book_id, format, library_path).await?;
    // Refresh the target's FTS row so this format's contribution to the
    // searchable text lands immediately (mirrors the ebook attach path).
    upsert_fts(tx, book_id).await?;
    if let Some(cover) = attach::maybe_adopt_cover(tx, book_id, b.cover.as_ref()).await? {
        covers.push(cover);
    }
    Ok(())
}

/// Return type from a fresh audiobook insert — both ids needed by the
/// caller for the parts + FTS + author-link inserts.
struct InsertedAudiobook {
    book_id: i64,
    book_file_id: i64,
}

/// Insert the `books` row and its single `book_files` row for a new
/// audiobook, returning the ids of both.
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

    let book_id = sqlx::query_scalar::<_, i64>(
        "INSERT INTO books \
            (uuid, library_id, path, title, sort, author_sort, has_cover, description, \
             accent_color, title_norm, author_norm) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?) \
         RETURNING id",
    )
    .bind(&b.uuid)
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
    Ok(id)
}

/// Batch-insert `book_file_parts` rows for the ordered part list.
async fn insert_audiobook_parts(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_file_id: i64,
    parts: &[crate::audiobook::AudiobookPart],
) -> Result<(), sqlx::Error> {
    for p in parts {
        sqlx::query(
            "INSERT INTO book_file_parts \
                (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(book_file_id)
        .bind(p.ordinal)
        .bind(&p.filename)
        .bind(p.size_bytes)
        .bind(p.mtime_epoch)
        .bind(p.duration_seconds)
        .execute(&mut **tx)
        .await?;
    }
    Ok(())
}

/// Insert `file_chapters` rows. If no chapters were extracted, synthesize
/// one chapter per part so the frontend always gets `chapters.len() >= 1`.
pub(crate) async fn insert_chapters(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_file_id: i64,
    chapters: &[crate::audiobook::RawChapter],
    parts: &[crate::audiobook::AudiobookPart],
) -> Result<(), SyncError> {
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
            sqlx::query(
                "INSERT INTO file_chapters \
                    (book_file_id, ordinal, title, start_seconds, duration_seconds) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(book_file_id)
            .bind(i as i64)
            .bind(&ch.title)
            .bind(start_seconds)
            .bind(duration_seconds)
            .execute(&mut **tx)
            .await?;
        }
    } else {
        // Synthetic fallback: one chapter per part.
        let mut cumulative = 0.0f64;
        for p in parts {
            let title = format!("Part {}", p.ordinal + 1);
            sqlx::query(
                "INSERT INTO file_chapters \
                    (book_file_id, ordinal, title, start_seconds, duration_seconds) \
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(book_file_id)
            .bind(p.ordinal)
            .bind(&title)
            .bind(cumulative)
            .bind(p.duration_seconds)
            .execute(&mut **tx)
            .await?;
            cumulative += p.duration_seconds;
        }
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
            last_modified = datetime('now') \
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
