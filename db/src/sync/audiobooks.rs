//! Multi-file audiobook sync. Mirrors `sync_books` but writes
//! `book_file_parts` rows in addition to `books` and `book_files`.

use sqlx::{SqlitePool, Transaction};

use crate::covers::delete_cover_files_for;
use crate::settings::upsert_library;

use super::books::materialize_new_covers;

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
/// 1. Upsert `libraries` row.
/// 2. Delete Removed (explicit FTS clear + cascade DELETE from `books`).
/// 3. Update Changed in-place: wipe `book_files` + `book_file_parts` + author
///    link + FTS, then re-insert them.
/// 4. Insert New.
/// 5. Backfill `book_files.(mtime_epoch, size_bytes)` only.
/// 6. Stamp `libraries.last_indexed`.
///
/// Post-commit: write / delete cover files (best-effort, same as sync_books).
pub async fn sync_audiobooks(
    pool: &SqlitePool,
    library_path: &str,
    plan: AudiobookSyncPlan,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;
    let library_id = upsert_library(&mut tx, library_path).await?;

    // --- Removed ----------------------------------------------------------
    if !plan.removed_uuids.is_empty() {
        let placeholders = std::iter::repeat_n("?", plan.removed_uuids.len())
            .collect::<Vec<_>>()
            .join(", ");
        let fts_sql = format!(
            "DELETE FROM books_fts WHERE rowid IN \
             (SELECT id FROM books WHERE library_id = ? AND uuid IN ({placeholders}))"
        );
        let mut q = sqlx::query(&fts_sql).bind(library_id);
        for uuid in &plan.removed_uuids {
            q = q.bind(uuid);
        }
        q.execute(&mut *tx).await?;

        let books_sql =
            format!("DELETE FROM books WHERE library_id = ? AND uuid IN ({placeholders})");
        let mut q = sqlx::query(&books_sql).bind(library_id);
        for uuid in &plan.removed_uuids {
            q = q.bind(uuid);
        }
        q.execute(&mut *tx).await?;
    }

    // --- Changed ----------------------------------------------------------
    let mut changed_covers: Vec<(String, String, Vec<u8>)> = Vec::new();
    for b in &plan.changed_books {
        let Some(book_id) =
            sqlx::query_scalar::<_, i64>("SELECT id FROM books WHERE library_id = ? AND uuid = ?")
                .bind(library_id)
                .bind(&b.uuid)
                .fetch_optional(&mut *tx)
                .await?
        else {
            // TOCTOU: promote to New insert.
            let inserted = insert_audiobook_row(&mut tx, library_id, b).await?;
            insert_audiobook_parts(&mut tx, inserted.book_file_id, &b.parts).await?;
            insert_audiobook_author_link(&mut tx, inserted.book_id, b.creator_name.as_deref())
                .await?;
            insert_audiobook_fts_row(&mut tx, inserted.book_id, b).await?;
            if let Some((mime, bytes)) = &b.cover {
                changed_covers.push((b.uuid.clone(), mime.clone(), bytes.clone()));
            }
            continue;
        };

        update_audiobook_row(&mut tx, book_id, b).await?;
        // Wipe dependent rows; ON DELETE CASCADE handles book_file_parts when
        // book_files is deleted.
        sqlx::query("DELETE FROM books_authors_link WHERE book = ?")
            .bind(book_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM book_files WHERE book_id = ?")
            .bind(book_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("DELETE FROM books_fts WHERE rowid = ?")
            .bind(book_id)
            .execute(&mut *tx)
            .await?;

        let book_file_id = insert_audiobook_file_row(&mut tx, book_id, b).await?;
        insert_audiobook_parts(&mut tx, book_file_id, &b.parts).await?;
        insert_audiobook_author_link(&mut tx, book_id, b.creator_name.as_deref()).await?;
        insert_audiobook_fts_row(&mut tx, book_id, b).await?;

        if let Some((mime, bytes)) = &b.cover {
            changed_covers.push((b.uuid.clone(), mime.clone(), bytes.clone()));
        }
    }

    // --- New --------------------------------------------------------------
    let mut new_covers: Vec<(String, String, Vec<u8>)> = Vec::new();
    for b in &plan.new_books {
        let inserted = insert_audiobook_row(&mut tx, library_id, b).await?;
        insert_audiobook_parts(&mut tx, inserted.book_file_id, &b.parts).await?;
        insert_audiobook_author_link(&mut tx, inserted.book_id, b.creator_name.as_deref()).await?;
        insert_audiobook_fts_row(&mut tx, inserted.book_id, b).await?;
        if let Some((mime, bytes)) = &b.cover {
            new_covers.push((b.uuid.clone(), mime.clone(), bytes.clone()));
        }
    }

    // --- Backfill ---------------------------------------------------------
    for chunk in plan.backfill.chunks(250) {
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
        q.execute(&mut *tx).await?;
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    sqlx::query("UPDATE libraries SET last_indexed = ? WHERE id = ?")
        .bind(now)
        .bind(library_id)
        .execute(&mut *tx)
        .await?;

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
            (uuid, library_id, path, title, sort, author_sort, has_cover, description) \
         VALUES (?, ?, ?, ?, ?, ?, ?, ?) \
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
            (book_id, format, filename, size_bytes, mtime, mtime_epoch) \
         VALUES (?, ?, ?, ?, '', ?) \
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

/// Insert or replace the FTS row for an audiobook.
async fn insert_audiobook_fts_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    b: &crate::audiobook::IndexedAudiobook,
) -> Result<(), sqlx::Error> {
    let author_text = b.creator_name.as_deref().unwrap_or("");
    let desc_text = b.description.as_deref().unwrap_or("");
    sqlx::query(
        "INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn) \
         VALUES (?, ?, ?, '', '', ?, '')",
    )
    .bind(book_id)
    .bind(&b.title)
    .bind(author_text)
    .bind(desc_text)
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
            description = ?, last_modified = datetime('now') \
         WHERE id = ?",
    )
    .bind(&book_path)
    .bind(&b.title)
    .bind(&b.title)
    .bind(&b.creator_name)
    .bind(has_cover)
    .bind(&b.description)
    .bind(book_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
