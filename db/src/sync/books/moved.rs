//! Moved bucket: repoint the path columns of rows that already exist,
//! for files the pure diff matched on their `(size_bytes, mtime_epoch)`
//! pair. No parse, no insert, no delete — a relocation touches only
//! `books.(scan_key, path)`, `book_files.(filename, scan_key, path)`,
//! `book_file_parts.filename`, and the `merged_uuids` ledger. `books_fts`
//! indexes no path column, so it needs no refresh either.
//!
//! `book_files.format` is deliberately **not** in that list, and the
//! classifier is what makes that safe: `match_moved_files` keys on the
//! format alongside the stat pair, so a file whose extension changed never
//! reaches this writer. Rewriting `format` here instead would be worse than
//! the bug it papers over — it would claim a Phase-B re-parse happened when
//! none did, and `format` is exactly what `books::book_file_path` rebuilds
//! the on-disk path from.
//!
//! Shared by both pipelines: `sync_audiobooks` calls the same writer,
//! because a relocated group differs from a relocated ebook only in
//! having `book_file_parts` rows to carry along.

use sqlx::Transaction;

use crate::helpers::{scan_key_for, split_filename};

use super::super::attach;
use super::shared::clear_missing_files_flag;
use super::MovedFile;

/// A relocated file's new home, split once by the caller so each row
/// writer below binds a value rather than re-deriving one.
struct NewLocation<'a> {
    /// The new `books.scan_key` / `book_files.scan_key`.
    scan_key: &'a str,
    /// Parent directory — the new `books.path`, and the new
    /// `book_files.path` override on an attached row.
    dir: &'a str,
    /// Leaf stem — the new `book_files.filename` (extensionless, per the
    /// column's contract).
    stem: &'a str,
}

/// Relocate every file in the Moved bucket in place.
///
/// Each entry names one row by uuid, resolved in two steps: the book's own
/// `books` row first, then the `merged_uuids` ledger. Both projections feed
/// the diff, so both can surface here, and the two write different things —
/// an attached file's move must leave the target book's `scan_key` alone
/// (it names a *different* file).
///
/// A relocation whose destination is already spoken for is logged and
/// skipped per-book, on both branches: the `books` side would be refused by
/// the `idx_books_scan_key` unique index, and the ledger side has no such
/// index but must preserve the same uniqueness by hand (see
/// `attach::attachment_scan_key_taken`). Failing a whole reindex over one
/// ambiguous path would be worse than leaving the book where it is.
///
/// Skipping is stable, not self-healing, and that is the accepted cost:
/// the diff took the arrival out of New and the departure out of Removed
/// before the writer ever saw them, so a declined book keeps naming its old
/// path and the next scan re-offers — and re-declines — the same move,
/// while the file at the destination goes unindexed. It stays that way
/// until the squatting row is resolved. Reaching this needs a book whose
/// `scan_key` names a path its own format no longer occupies (its file went
/// missing while a different format's attachment kept it out of the ebook
/// projection) *and* a second file moved onto exactly that path.
pub(in crate::sync) async fn sync_moved(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    library_path: &str,
    moved: &[MovedFile],
) -> Result<(), sqlx::Error> {
    if moved.is_empty() {
        return Ok(());
    }
    let mut relocated = 0usize;
    for m in moved {
        if relocate_one(tx, library_id, library_path, m).await? {
            relocated += 1;
        }
    }
    tracing::info!(
        relocated,
        skipped = moved.len() - relocated,
        "sync: relocated moved files in place"
    );
    Ok(())
}

/// Relocate the single row `m.uuid` names. `Ok(false)` when the move was
/// declined (unresolvable uuid, or a destination another book already
/// holds) — declining writes nothing at all, so the book keeps naming its
/// old path and the next scan re-offers the same move.
async fn relocate_one(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    library_path: &str,
    m: &MovedFile,
) -> Result<bool, sqlx::Error> {
    let scan_key = scan_key_for(&m.filename);
    let (dir, stem, _) = split_filename(&m.filename);
    let to = NewLocation {
        scan_key: &scan_key,
        dir: &dir,
        stem: &stem,
    };

    if let Some((book_id, old_scan_key)) = anchor_row(tx, library_id, &m.uuid).await? {
        if scan_key_taken(tx, library_id, to.scan_key, book_id).await? {
            tracing::warn!(
                uuid = %m.uuid,
                new_scan_key = %to.scan_key,
                "sync: skipping relocation — another book under this scan root already \
                 holds the destination path (idx_books_scan_key)"
            );
            return Ok(false);
        }
        // `books` carries the unique index, so it moves first: if it can't
        // land, nothing else has been written and the row stays coherent.
        // `last_modified` is deliberately *not* bumped — a relocation changes
        // no byte a client can see, and bumping it would push a whole
        // reorganized library through every client's changed-since feed.
        sqlx::query("UPDATE books SET scan_key = ?, path = ? WHERE id = ?")
            .bind(to.scan_key)
            .bind(to.dir)
            .bind(book_id)
            .execute(&mut **tx)
            .await?;
        // A native row's `path` is NULL and inherits `books.path`, which
        // just moved — so only refresh an override this row already carries.
        relocate_file_row(tx, book_id, &old_scan_key, &to, false).await?;
        // The book held its file throughout, so this is a repair, not a
        // re-gain: a no-op unless an earlier scan left the flag set.
        clear_missing_files_flag(tx, book_id).await?;
        return Ok(true);
    }

    let Some((book_id, old_scan_key)) =
        attach::find_attachment_by_uuid(tx, library_path, &m.uuid).await?
    else {
        tracing::warn!(
            uuid = %m.uuid,
            "sync: moved uuid names neither a books row nor an attachment — skipping"
        );
        return Ok(false);
    };
    if attach::attachment_scan_key_taken(tx, library_path, to.scan_key, &m.uuid).await? {
        tracing::warn!(
            uuid = %m.uuid,
            new_scan_key = %to.scan_key,
            "sync: skipping relocation — another ledger row already holds the \
             destination path under this scan root"
        );
        return Ok(false);
    }
    // An attachment's identity is its ledger row; the target book's own
    // `scan_key` / `path` name a different file and must not move. Its
    // `book_files.path` override is therefore the *only* column pointing at
    // the file, so it must be written whether or not it was set before.
    // Ledger first, mirroring the `books` branch: it carries the uniqueness
    // this path has to preserve by hand, so nothing else is written until it
    // has been shown to land.
    attach::relocate_attachment(tx, &m.uuid, to.scan_key).await?;
    relocate_file_row(tx, book_id, &old_scan_key, &to, true).await?;
    Ok(true)
}

/// The `books` row `uuid` names under this scan root, with its current
/// `scan_key`. `None` when the uuid isn't a book's own identity (it's a
/// ledger uuid) or the row carries no `scan_key` — without one there is no
/// `book_files` row to find, since both diff projections join on it.
async fn anchor_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    uuid: &str,
) -> Result<Option<(i64, String)>, sqlx::Error> {
    let row: Option<(i64, String)> = sqlx::query_as(
        "SELECT id, COALESCE(scan_key, '') FROM books WHERE library_id = ? AND uuid = ?",
    )
    .bind(library_id)
    .bind(uuid)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(row.filter(|(_, scan_key)| !scan_key.is_empty()))
}

/// Would moving `book_id` onto `new_scan_key` collide with the
/// `idx_books_scan_key` unique index? Checked rather than caught: the diff
/// can't see a book scoped out of its own format projection (a book whose
/// only remaining file is an M4B still owns its `.epub` scan_key), so this
/// is reachable, and a pre-check keeps the transaction clean.
///
/// The holder is always a **stationary** row, never another entry in this
/// same bucket, so skipping cannot deadlock a swap or a cycle. Two things
/// rule that out: each arrival is matched at most once, so no two entries
/// share a destination; and a departure requires its `scan_key` to be
/// absent from disk while an arrival requires its path to be present, so an
/// entry's destination can never be another entry's source. A real
/// path swap leaves both paths occupied, so neither row is a departure and
/// both classify Changed — see
/// `diff_classifies_a_path_swap_as_changed_never_as_a_moved_cycle`.
async fn scan_key_taken(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    library_id: i64,
    new_scan_key: &str,
    book_id: i64,
) -> Result<bool, sqlx::Error> {
    let hit: Option<i64> = sqlx::query_scalar(
        "SELECT 1 FROM books WHERE library_id = ? AND scan_key = ? AND id <> ? LIMIT 1",
    )
    .bind(library_id)
    .bind(new_scan_key)
    .bind(book_id)
    .fetch_optional(&mut **tx)
    .await?;
    Ok(hit.is_some())
}

/// Repoint the `book_files` row keyed on `(book_id, old_scan_key)` — the
/// file that actually moved — plus any `book_file_parts` hanging off it.
///
/// `path` is a location *override* (migration 0016): NULL means "inherit
/// `books.path`". `force_override` says which side of that the caller is
/// on — an attachment must always name its own directory, a native row
/// must keep inheriting unless it already carried an override.
async fn relocate_file_row(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    old_scan_key: &str,
    to: &NewLocation<'_>,
    force_override: bool,
) -> Result<(), sqlx::Error> {
    let row: Option<(i64, i64)> = sqlx::query_as(
        "SELECT id, path IS NOT NULL FROM book_files WHERE book_id = ? AND scan_key = ?",
    )
    .bind(book_id)
    .bind(old_scan_key)
    .fetch_optional(&mut **tx)
    .await?;
    let Some((book_file_id, has_override)) = row else {
        // Another bucket in this same transaction took the row (the Changed
        // path wipes a book's rows per format before re-inserting). The
        // ledger / `books` side still moved, so the next scan reclassifies
        // the file and re-creates the row under its new path.
        tracing::warn!(
            book_id,
            old_scan_key,
            "sync: no book_files row to relocate — leaving the file for the next scan"
        );
        return Ok(());
    };
    if force_override || has_override != 0 {
        sqlx::query("UPDATE book_files SET filename = ?, scan_key = ?, path = ? WHERE id = ?")
            .bind(to.stem)
            .bind(to.scan_key)
            .bind(to.dir)
            .bind(book_file_id)
            .execute(&mut **tx)
            .await?;
    } else {
        sqlx::query("UPDATE book_files SET filename = ?, scan_key = ? WHERE id = ?")
            .bind(to.stem)
            .bind(to.scan_key)
            .bind(book_file_id)
            .execute(&mut **tx)
            .await?;
    }
    relocate_file_parts(tx, book_file_id, old_scan_key, to.scan_key).await
}

/// Carry an audiobook group's `book_file_parts` along with it. Part
/// filenames are **library-relative** — the HLS pipeline joins them onto
/// the scan root, not onto `books.path` (see `build_concat_file`) — so each
/// is the group's own scan_key plus whatever follows it inside the group; a
/// single-file group's one part *is* the group path. Ebook rows have no
/// parts, so this reads zero rows off the `UNIQUE(book_file_id, ordinal)`
/// index and returns.
///
/// Done in Rust rather than one `substr`-based UPDATE so a part that
/// doesn't actually sit under its group is skipped and logged instead of
/// having a path invented for it.
async fn relocate_file_parts(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_file_id: i64,
    old_group: &str,
    new_group: &str,
) -> Result<(), sqlx::Error> {
    let parts: Vec<(i64, String)> =
        sqlx::query_as("SELECT id, filename FROM book_file_parts WHERE book_file_id = ?")
            .bind(book_file_id)
            .fetch_all(&mut **tx)
            .await?;
    for (id, filename) in parts {
        let Some(new_filename) = rebase_part(&filename, old_group, new_group) else {
            tracing::warn!(
                book_file_id,
                filename,
                old_group,
                "sync: part does not sit under its group — not relocated"
            );
            continue;
        };
        sqlx::query("UPDATE book_file_parts SET filename = ? WHERE id = ?")
            .bind(&new_filename)
            .bind(id)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// Swap a part's `old_group` prefix for `new_group`. `None` when the part
/// isn't the group itself or a path inside it — the separator check is what
/// stops a group at `A/Book` from swallowing `A/Bookshelf/01.mp3`.
fn rebase_part(filename: &str, old_group: &str, new_group: &str) -> Option<String> {
    let rest = filename.strip_prefix(old_group)?;
    if rest.is_empty() || rest.starts_with('/') {
        Some(format!("{new_group}{rest}"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests;
