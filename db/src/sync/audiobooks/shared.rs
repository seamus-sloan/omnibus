//! Cross-bucket helpers shared by `new` and `changed`: canonical row
//! writers (`insert_audiobook_row` / `update_audiobook_row` /
//! `insert_audiobook_file_row` / `insert_audiobook_parts` /
//! `insert_chapters`), the rewrite-in-place and cross-format attach paths,
//! and the author-link writer.

use sqlx::Transaction;

use crate::helpers::{mint_uuid, sanitize_accent_color, stable_uuid};
use crate::normalize::{normalize_author, normalize_title};

use super::super::attach;
use super::super::books::SyncError;
use super::super::fts::upsert_fts;

/// Rewrite an existing audiobook in place from a freshly-parsed group,
/// preserving `books.id`/`books.uuid`: refresh the `books` scalars, wipe +
/// re-insert this format's `book_files`/parts/chapters and the author link,
/// and refresh FTS. Shared by the Changed update path and the New re-attach
/// path (a fileless book whose group returned, or a `replace_books`
/// re-add). The `book_files` delete is scoped to this group's own format so
/// a cross-format attachment (e.g. an EPUB row via `merged_uuids`) survives.
pub(super) async fn rewrite_audiobook_in_place(
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
    super::super::push_cover(covers, uuid, &b.cover);
    Ok(())
}

/// Insert a fresh audiobook (canonical `books` + `book_files` + parts +
/// chapters + author-link + FTS row) and push its cover triple if any.
/// Shared by `sync_audiobooks_new` and the TOCTOU promote inside
/// `sync_audiobooks_changed`.
pub(super) async fn insert_new_audiobook(
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
    super::super::push_cover(covers, &inserted.uuid, &b.cover);
    Ok(())
}

/// Try to attach a brand-new audiobook group to an existing book in
/// another format instead of inserting a fresh `books` row. Mirrors
/// `books::try_attach_new_ebook`: a `merged_uuids` hit attaches
/// unconditionally; otherwise exactly one normalized title+author match
/// without this format attaches. Returns `true` when attached.
pub(super) async fn try_attach_new_audiobook(
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
        if attach_audiobook_file(tx, target_id, &format, library_path, b, covers).await? {
            return Ok(true);
        }
        // Slot taken by a different file: drop this file's stale ledger row so
        // it stops replaying, and fall through to insert it as its own book.
        attach::forget_attachment(tx, library_path, &b.scan_key).await?;
        return Ok(false);
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
    // find_attach_target excludes a book that already has this format's file
    // row, but the writer re-checks the ledger and may still refuse a slot a
    // different file holds — return whatever it decides.
    attach_audiobook_file(tx, target_id, &b.format, library_path, b, covers).await
}

/// Write (or rewrite) an attached audiobook's `book_files` row (plus
/// parts and chapters) under `book_id`, record the attachment, and adopt
/// the cover when the target has none. The target's `books` scalars and
/// links are deliberately left untouched, but its FTS row is refreshed
/// via the door so any newly-unioned text becomes searchable. The file
/// row carries its own `(library_path, path)` location override — the
/// target book may live in a different library, and the HLS read path
/// resolves part filenames against the *audio* root.
///
/// Returns `Ok(false)` without writing when another file holds the slot.
pub(super) async fn attach_audiobook_file(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    format: &str,
    library_path: &str,
    b: &crate::audiobook::IndexedAudiobook,
    covers: &mut Vec<(String, String, Vec<u8>)>,
) -> Result<bool, SyncError> {
    // One attached file per (book, format) slot: refuse rather than delete a
    // different file's row (the DELETE below is scoped only to the format).
    if attach::slot_held_by_other(tx, book_id, format, &b.scan_key).await? {
        return Ok(false);
    }
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
    Ok(true)
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
    super::super::books::clear_missing_files_flag(tx, book_id).await?;
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
