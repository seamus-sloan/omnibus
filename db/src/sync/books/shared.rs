//! Cross-bucket helpers shared by `sync_new` and `sync_changed`:
//! canonical row writers (`insert_book_row` / `update_book_row` /
//! `insert_book_file_row`), metadata + tag + identifier link dispatch,
//! the rewrite-in-place and cross-format attach paths, and the
//! post-commit cover materialization + missing-files marker helper.

use std::collections::HashSet;

use omnibus_shared::{CleanupKind, EbookMetadata};
use sqlx::Transaction;

use crate::covers::write_cover_file;
use crate::entity_alias::resolve_entity_aliases;
use crate::helpers::{
    cleaned_series_name, mint_uuid, resolved_series_index, sanitize_accent_color, scan_key_for,
    split_filename, stable_uuid,
};
use crate::normalize::{normalize_author, normalize_title};
use crate::sort_keys::series_sort_value;
use crate::taxonomy::{
    resolve_or_insert_language, resolve_or_insert_publisher, resolve_or_insert_series,
};

use super::super::attach;
use super::super::authors::insert_author_links;
use super::super::fts::upsert_fts;
use super::wipe_per_book_link_rows;

/// Rewrite an existing book in place from a freshly-parsed entry: refresh
/// the `books` scalars, wipe + re-insert this format's `book_files` and the
/// per-book links, and refresh FTS — preserving `books.id`/`books.uuid`.
/// Shared by the Changed update path and the New re-attach path.
pub(super) async fn rewrite_book_in_place(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    uuid: &str,
    b: &crate::ebook::IndexedBook,
    covers: &mut Vec<(String, String, Vec<u8>)>,
) -> Result<(), sqlx::Error> {
    update_book_row(tx, book_id, b).await?;
    let (_, _, file_ext) = split_filename(&b.metadata.filename);
    wipe_per_book_link_rows(tx, book_id, &file_ext).await?;
    insert_book_file_row(tx, book_id, b).await?;
    insert_metadata_links(tx, book_id, &b.metadata).await?;
    upsert_fts(tx, book_id).await?;
    super::super::push_cover(covers, uuid, &b.cover);
    Ok(())
}

/// Try to attach a brand-new ebook file to an existing book in another
/// format instead of inserting a fresh `books` row. Two triggers, in
/// order: (1) the file's uuid is already in `merged_uuids` (it was
/// attached or merged before — the "this was merged" path, which works
/// even when titles no longer match); (2) exactly one existing book
/// matches on normalized title + author and lacks this format. Returns
/// `true` when the file was attached (the caller skips its normal
/// insert). Per-file parse errors never attach — their metadata is a
/// filename fallback, not a real title.
///
/// `removed_this_scan` is the set of uuids this same sync just dropped from
/// the Removed bucket. When the (2) match lands on one of them, the file
/// isn't a genuine cross-format attachment — it's that book's own native
/// file returning under a new path (a relocation) — so it's
/// rewritten in place (updating `books.scan_key`) instead of minting a
/// `merged_uuids` row.
pub(super) async fn try_attach_new_ebook(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_path: &str,
    b: &crate::ebook::IndexedBook,
    removed_this_scan: &HashSet<&str>,
    covers: &mut Vec<(String, String, Vec<u8>)>,
) -> Result<bool, sqlx::Error> {
    if b.metadata.error.is_some() {
        return Ok(false);
    }
    let m = &b.metadata;
    let scan_key = scan_key_for(&m.filename);
    let (_, _, file_ext) = split_filename(&m.filename);

    // Already a recorded attachment? Match by the repoint-stable relative path
    // and re-attach against the stored ledger uuid.
    if let Some((merged_uuid, target_id, format)) =
        attach::find_attachment_by_scan_key(tx, library_path, &scan_key).await?
    {
        if attach_ebook_file(
            tx,
            target_id,
            &format,
            library_path,
            &merged_uuid,
            b,
            covers,
        )
        .await?
        {
            return Ok(true);
        }
        // Slot taken by a different file: drop this file's stale ledger row so
        // it stops replaying, and fall through to insert it as its own book.
        attach::forget_attachment(tx, library_path, &scan_key).await?;
        return Ok(false);
    }

    let title = m.display_title();
    let (Some(title_norm), Some(author_norm)) = (
        normalize_title(&title),
        m.creators.first().and_then(|c| normalize_author(&c.name)),
    ) else {
        // No author (or empty title): too weak a signal to auto-match.
        return Ok(false);
    };
    let Some((target_id, target_uuid)) =
        attach::find_attach_target(tx, &title_norm, &author_norm, &file_ext).await?
    else {
        return Ok(false);
    };
    if removed_this_scan.contains(target_uuid.as_str()) {
        rewrite_book_in_place(tx, target_id, &target_uuid, b, covers).await?;
        return Ok(true);
    }
    // A brand-new attachment: mint a stable handle for the ledger row (the
    // lookup above is by scan_key, so this uuid is only an identifier).
    let uuid = stable_uuid(library_path, &m.filename);
    // find_attach_target excludes a book that already has this format's file
    // row, but the writer re-checks the ledger and may still refuse a slot a
    // different file holds — return whatever it decides.
    attach_ebook_file(tx, target_id, &file_ext, library_path, &uuid, b, covers).await
}

/// Write (or rewrite) an attached ebook's `book_files` row under
/// `book_id`, record the attachment, adopt the cover when the target has
/// none, and union the file's identifiers (target's values win). The
/// target's `books` scalars and links are deliberately left untouched —
/// target metadata wins — but the FTS row is refreshed via the door so
/// the newly-unioned identifiers (incl. an attached-only ISBN) become
/// searchable immediately.
///
/// Returns `Ok(false)` without writing when another file holds the slot.
pub(super) async fn attach_ebook_file(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    format: &str,
    library_path: &str,
    uuid: &str,
    b: &crate::ebook::IndexedBook,
    covers: &mut Vec<(String, String, Vec<u8>)>,
) -> Result<bool, sqlx::Error> {
    // The attached file's own relative path is its diff key (F2), so a repoint
    // of the file's scan root re-matches it instead of resurfacing it as a
    // duplicate; it's also the ledger key written below.
    let scan_key = scan_key_for(&b.metadata.filename);
    // One attached file per (book, format) slot: refuse rather than delete a
    // different file's row (the DELETE below is scoped only to the format).
    if attach::slot_held_by_other(tx, book_id, format, &scan_key).await? {
        return Ok(false);
    }
    // Idempotent re-attach: the refresh path (and the self-healing "uuid
    // known but attachment row missing" path) both land here.
    sqlx::query("DELETE FROM book_files WHERE book_id = ? AND format = ?")
        .bind(book_id)
        .bind(format)
        .execute(&mut **tx)
        .await?;
    insert_book_file_row(tx, book_id, b).await?;
    // Location override: the attached file's on-disk home is its own
    // `(library root, dir)`, not the target book's (migration 0016).
    let (file_dir, _, _) = split_filename(&b.metadata.filename);
    sqlx::query(
        "UPDATE book_files SET library_path = ?, path = ? WHERE book_id = ? AND format = ?",
    )
    .bind(library_path)
    .bind(&file_dir)
    .bind(book_id)
    .bind(format)
    .execute(&mut **tx)
    .await?;
    insert_identifier_links(tx, book_id, &b.metadata).await?;
    attach::record_attachment(tx, uuid, book_id, format, library_path, &scan_key).await?;
    // The unioned identifiers (incl. a new ISBN from this format) just
    // changed the target's searchable text — refresh its FTS row.
    upsert_fts(tx, book_id).await?;
    if let Some(cover) = attach::maybe_adopt_cover(tx, book_id, b.cover.as_ref()).await? {
        covers.push(cover);
    }
    Ok(true)
}

/// Insert the `book_files` row for an existing `books.id`. Shared by the
/// Changed re-insert path; the New path calls this indirectly via
/// `insert_book_row`.
async fn insert_book_file_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    b: &crate::ebook::IndexedBook,
) -> Result<(), sqlx::Error> {
    let m = &b.metadata;
    let (_, file_stem, file_ext) = split_filename(&m.filename);
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch, scan_key)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(book_id)
    .bind(&file_ext)
    .bind(&file_stem)
    .bind(b.size_bytes)
    .bind(b.mtime_epoch)
    // Per-file attachment identity: the file's own relative path (F2), so a
    // cross-format attachment resolves to its own row on the shared merged join.
    .bind(scan_key_for(&m.filename))
    .execute(&mut **tx)
    .await?;
    clear_missing_files_flag(tx, book_id).await?;
    Ok(())
}

/// Clear the F10 missing-files flag for a book that just (re)gained a file —
/// the Changed/New file-write chokepoint. Guarded on `is_missing_files = 1` so
/// it's a no-op for the common already-attached insert.
pub(in crate::sync) async fn clear_missing_files_flag(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "UPDATE books SET is_missing_files = 0, missing_files_since = NULL
          WHERE id = ? AND is_missing_files = 1",
    )
    .bind(book_id)
    .execute(&mut **tx)
    .await?;
    Ok(())
}

/// UPDATE the `books` row for a Changed entry in place (preserving id).
/// All scalar columns that `insert_book_row` writes get refreshed; the
/// link tables and FTS row are handled by the caller.
///
/// Also refreshes `scan_key` from `b`'s own filename. For the Changed bucket
/// and the New-bucket same-scan_key rewrite this is a no-op (the entry
/// already matched on that value); it's load-bearing for the New-bucket
/// relocation rewrite, where the incoming file's path is the book's *new*
/// scan_key.
async fn update_book_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    b: &crate::ebook::IndexedBook,
) -> Result<(), sqlx::Error> {
    let m = &b.metadata;
    let (book_path, _, _) = split_filename(&m.filename);
    let scan_key = scan_key_for(&m.filename);
    let title = m.display_title();
    let series_index_num = resolved_series_index(m);
    let author_sort = m
        .creators
        .first()
        .and_then(|c| c.file_as.clone())
        .or_else(|| m.creators.first().map(|c| c.name.clone()));
    let has_cover = i64::from(b.cover.is_some());

    sqlx::query(
        "UPDATE books SET
            scan_key = ?, path = ?, title = ?, sort = ?, author_sort = ?, series_sort = ?,
            series_index = ?, pubdate = ?, has_cover = ?, description = ?, accent_color = ?,
            title_norm = ?, author_norm = ?, word_count = ?, page_count = ?,
            last_modified = strftime('%s','now')
         WHERE id = ?",
    )
    .bind(&scan_key)
    .bind(&book_path)
    .bind(&title)
    .bind(&title)
    .bind(&author_sort)
    .bind(series_sort_value(m))
    .bind(series_index_num)
    .bind(&m.published)
    .bind(has_cover)
    .bind(&m.description)
    .bind(sanitize_accent_color(m.accent.as_deref()))
    .bind(normalize_title(&title))
    .bind(m.creators.first().and_then(|c| normalize_author(&c.name)))
    .bind(b.word_count)
    .bind(m.page_count)
    .bind(book_id)
    .execute(&mut **tx)
    .await?;

    Ok(())
}

/// Fields the per-book outer loop needs after the canonical `books` /
/// `book_files` inserts have run. The FTS row is sourced from the written
/// rows via [`upsert_fts`], so the loop only needs the id (for the upsert
/// + link writes) and the uuid (for the post-commit cover triple).
pub(super) struct InsertedBook {
    pub(super) book_id: i64,
    pub(super) uuid: String,
}

/// Insert the canonical `books` row (returning its id) and its single
/// `book_files` row.
pub(super) async fn insert_book_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    // F2: identity is minted, not path-derived, so `library_path` no longer
    // participates in the insert (the scan_key comes from the filename).
    _library_path: &str,
    b: &crate::ebook::IndexedBook,
) -> Result<InsertedBook, sqlx::Error> {
    let m = &b.metadata;
    // F2: identity is a fresh, durable v4 minted once here and never
    // recomputed; the diff matches on `scan_key` (the relative path).
    let uuid = mint_uuid();
    let scan_key = scan_key_for(&m.filename);
    let (book_path, file_stem, file_ext) = split_filename(&m.filename);
    let title = m.display_title();
    let series_index_num = resolved_series_index(m);
    let author_sort = m
        .creators
        .first()
        .and_then(|c| c.file_as.clone())
        .or_else(|| m.creators.first().map(|c| c.name.clone()));
    let has_cover = i64::from(b.cover.is_some());

    let book_id = sqlx::query_scalar::<_, i64>(
        // `timestamp`/`last_modified` are set explicitly: migration 0038
        // converted them in place to INTEGER and (unlike a recreate) could not
        // carry the old `DEFAULT (strftime('%s','now'))` forward.
        "INSERT INTO books
            (uuid, scan_key, library_id, path, title, sort, author_sort, series_sort, series_index,
             pubdate, has_cover, description, accent_color, title_norm, author_norm, word_count,
             page_count, timestamp, last_modified)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                 strftime('%s','now'), strftime('%s','now'))
         RETURNING id",
    )
    .bind(&uuid)
    .bind(&scan_key)
    .bind(library_id)
    .bind(&book_path)
    .bind(&title)
    .bind(&title)
    .bind(&author_sort)
    .bind(series_sort_value(m))
    .bind(series_index_num)
    .bind(&m.published)
    .bind(has_cover)
    .bind(&m.description)
    .bind(sanitize_accent_color(m.accent.as_deref()))
    .bind(normalize_title(&title))
    .bind(m.creators.first().and_then(|c| normalize_author(&c.name)))
    .bind(b.word_count)
    .bind(m.page_count)
    .fetch_one(&mut **tx)
    .await?;

    // `mtime_epoch INTEGER` holds the filesystem stat the incremental diff
    // compares against (migration 0009).
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch, scan_key)
         VALUES (?, ?, ?, ?, ?, ?)",
    )
    .bind(book_id)
    .bind(&file_ext)
    .bind(&file_stem)
    .bind(b.size_bytes)
    .bind(b.mtime_epoch)
    // Stamp scan_key on insert, matching `insert_book_file_row`'s sibling write.
    .bind(&scan_key)
    .execute(&mut **tx)
    .await?;

    Ok(InsertedBook { book_id, uuid })
}

/// Insert the per-book metadata join rows (authors + contributors, series,
/// tags, publisher, language, identifiers).
///
/// The multi-valued relations (authors, tags, identifiers) are written in
/// batches rather than one statement per term: collect the distinct terms
/// for this book, `INSERT OR IGNORE` them all in one statement, then write
/// the join rows with one `INSERT ... SELECT ... JOIN` that resolves each id
/// inline by name (NOCASE) — no separate id-resolution `SELECT`. Each batched
/// statement is chunked to stay under SQLite's bound-parameter limit. This
/// collapses the old ~4-queries-per-author / 2-queries-per-tag fan-out into a
/// constant handful per book, which keeps the SQLite write lock from being
/// held for the whole of a bulk import. Series / publisher / language are
/// single-valued per book, so they keep the simple resolve-then-link path.
pub(super) async fn insert_metadata_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    m: &EbookMetadata,
) -> Result<(), sqlx::Error> {
    insert_author_links(tx, book_id, m).await?;

    // Cleaned (trimmed, embedded-index-stripped — #1912) so the linked
    // `series.name` matches the `series_sort` denormalized onto the row
    // (`series_sort_value`) — keeps the sort key and the link in lockstep,
    // dedups whitespace variants, and collapses "Name #1"/"Name #2" onto one
    // series row instead of fragmenting.
    if let Some(series_name) = cleaned_series_name(m) {
        let series_id = resolve_or_insert_series(tx, &series_name).await?;
        sqlx::query("INSERT OR IGNORE INTO books_series_link (book, series) VALUES (?, ?)")
            .bind(book_id)
            .bind(series_id)
            .execute(&mut **tx)
            .await?;
    }

    insert_tag_links(tx, book_id, m).await?;

    if let Some(pub_name) = m.publisher.as_deref().filter(|s| !s.is_empty()) {
        let pub_id = resolve_or_insert_publisher(tx, pub_name).await?;
        sqlx::query("INSERT OR IGNORE INTO books_publishers_link (book, publisher) VALUES (?, ?)")
            .bind(book_id)
            .bind(pub_id)
            .execute(&mut **tx)
            .await?;
    }

    if let Some(lang_code) = m.language.as_deref().filter(|s| !s.is_empty()) {
        let lang_id = resolve_or_insert_language(tx, lang_code).await?;
        sqlx::query("INSERT OR IGNORE INTO books_languages_link (book, language) VALUES (?, ?)")
            .bind(book_id)
            .bind(lang_id)
            .execute(&mut **tx)
            .await?;
    }

    insert_identifier_links(tx, book_id, m).await?;

    Ok(())
}

/// Batch-insert the book's tag (subject) join rows: one `INSERT OR IGNORE`
/// into `tags` for all distinct non-empty subjects, then one link insert that
/// resolves ids via a NOCASE join. A subject a completed library-cleanup
/// merge already absorbed (#964) skips the `tags` insert and links straight
/// to its `entity_aliases` canonical id instead, so reindexing a file that
/// still names the merged-away tag can't resurrect it.
async fn insert_tag_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    m: &EbookMetadata,
) -> Result<(), sqlx::Error> {
    let mut seen = std::collections::HashSet::new();
    let tags: Vec<&str> = m
        .subjects
        .iter()
        .map(String::as_str)
        .filter(|s| !s.is_empty())
        .filter(|s| seen.insert(*s))
        .collect();
    if tags.is_empty() {
        return Ok(());
    }

    let aliased = resolve_entity_aliases(tx, CleanupKind::Tag, &tags).await?;
    let to_insert: Vec<&str> = tags
        .iter()
        .copied()
        .filter(|t| !aliased.contains_key(*t))
        .collect();

    // Both statements bind ~1 param per tag; chunk so a tag-heavy book can't
    // exceed SQLite's bound-parameter cap (999 by default). 500 keeps the link
    // statement (book_id + one per tag) safely under the limit.
    for chunk in to_insert.chunks(500) {
        let rows = std::iter::repeat_n("(?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let insert_sql = format!("INSERT OR IGNORE INTO tags (name) VALUES {rows}");
        let mut insert_q = sqlx::query(&insert_sql);
        for t in chunk {
            insert_q = insert_q.bind(*t);
        }
        insert_q.execute(&mut **tx).await?;

        let link_sql = format!(
            "INSERT OR IGNORE INTO books_tags_link (book, tag) \
             SELECT ?, t.id FROM (VALUES {rows}) AS v JOIN tags t ON t.name = v.column1"
        );
        let mut link_q = sqlx::query(&link_sql).bind(book_id);
        for t in chunk {
            link_q = link_q.bind(*t);
        }
        link_q.execute(&mut **tx).await?;
    }

    link_aliased_tags(tx, book_id, &aliased).await
}

/// Link the book straight to each already-known canonical tag id, for
/// subjects [`insert_tag_links`] found in `entity_aliases`.
async fn link_aliased_tags(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    aliased: &std::collections::HashMap<String, i64>,
) -> Result<(), sqlx::Error> {
    if aliased.is_empty() {
        return Ok(());
    }
    // Each row binds 2 params (book_id, tag_id), so chunk at 499 to stay
    // under SQLite's 999-parameter cap (499 * 2 = 998).
    let canonical_ids: Vec<i64> = aliased.values().copied().collect();
    for chunk in canonical_ids.chunks(499) {
        let rows = std::iter::repeat_n("(?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!("INSERT OR IGNORE INTO books_tags_link (book, tag) VALUES {rows}");
        let mut q = sqlx::query(&sql);
        for tag_id in chunk {
            q = q.bind(book_id).bind(*tag_id);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Batch-insert the book's identifiers in one statement. `INSERT OR IGNORE`
/// keeps every distinct `(book_id, scheme, value)` tuple — a book carrying
/// both an ISBN-10 and an ISBN-13 keeps both — and silently drops an exact
/// duplicate tuple (the same OPF listing one identifier twice). The wider
/// `(book_id, scheme, value)` PK (migration 0022) makes this lossless; the
/// cross-format attach path shares this writer, so an attached file's
/// identifiers union into the target rather than clobbering it.
async fn insert_identifier_links(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    m: &EbookMetadata,
) -> Result<(), sqlx::Error> {
    let idents: Vec<(&str, &str)> = m
        .identifiers
        .iter()
        .filter(|i| !i.value.is_empty())
        .map(|i| (i.scheme.as_deref().unwrap_or("unknown"), i.value.as_str()))
        .collect();
    if idents.is_empty() {
        return Ok(());
    }
    // 3 bound params per identifier; chunk at 250 (→ 750 binds) to stay under
    // SQLite's 999-param cap for books with very large identifier lists.
    // OR IGNORE keeps every distinct (scheme, value) and dedups exact-duplicate
    // tuples; chunk order is irrelevant since the PK now covers `value`.
    for chunk in idents.chunks(250) {
        let rows = std::iter::repeat_n("(?, ?, ?)", chunk.len())
            .collect::<Vec<_>>()
            .join(", ");
        let sql = format!(
            "INSERT OR IGNORE INTO book_identifiers (book_id, scheme, value) VALUES {rows}"
        );
        let mut q = sqlx::query(&sql);
        for (scheme, value) in chunk {
            q = q.bind(book_id).bind(*scheme).bind(*value);
        }
        q.execute(&mut **tx).await?;
    }
    Ok(())
}

/// Write the cover bytes that accompanied a successful sync transaction.
/// Filesystem side-effect, so deliberately split out of the transactional
/// path — failures are logged, not fatal.
pub(in crate::sync) fn materialize_new_covers(new_covers: Vec<(String, String, Vec<u8>)>) {
    for (uuid, mime, bytes) in new_covers {
        if let Err(e) = write_cover_file(&uuid, &mime, &bytes) {
            tracing::error!(
                error = %e,
                uuid = %uuid,
                "sync_books: failed to write cover"
            );
        }
    }
}
