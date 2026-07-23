//! Create a fileless book from external metadata — a `books` row with a
//! durable uuid, author links, identifiers, and a cover, but no `book_files`.
//! Used when a scanned/manually-entered physical book isn't in the library yet.

use sqlx::{SqlitePool, Transaction};

use super::PhysicalError;
use crate::covers::write_cover_file;
use crate::helpers::mint_uuid;
use crate::normalize::{normalize_author, normalize_title};

/// Sentinel scan-root path that owns every fileless (physical-only) book. It is
/// never a scan target, so ebook/audiobook reindex diffs never touch these
/// rows; the never-prune guard keeps the row alive while any physical book
/// references it.
const PHYSICAL_LIBRARY_PATH: &str = "physical://local";

/// A cover image for a fileless book: the source `mime` plus raw bytes.
pub struct FilelessCover {
    pub mime: String,
    pub bytes: Vec<u8>,
}

/// External metadata for a physical-only book. `pubdate` is a free-text year
/// (matching `books.pubdate`); there is no page-count column yet, so page data
/// is not persisted here.
pub struct FilelessBook {
    pub title: String,
    pub authors: Vec<String>,
    pub isbn: Option<String>,
    pub pubdate: Option<String>,
    pub description: Option<String>,
    pub cover: Option<FilelessCover>,
}

/// Create a fileless book and return its minted uuid.
///
/// Inserts the `books` row under the synthetic Physical scan root, links each
/// author, writes an `ISBN` identifier when present, and stores the cover via
/// the covers pipeline — all in one transaction. A cover-write failure rolls
/// the row back rather than leaving `has_cover` set with no file.
pub async fn create_fileless_book(
    pool: &SqlitePool,
    book: FilelessBook,
) -> Result<String, PhysicalError> {
    let mut tx = pool.begin().await?;

    let library_id = ensure_physical_library(&mut tx).await?;
    let uuid = mint_uuid();
    let has_cover = i64::from(book.cover.is_some());
    let author_sort = book.authors.first().cloned();
    let author_norm = book.authors.first().and_then(|a| normalize_author(a));

    let book_id = sqlx::query_scalar::<_, i64>(
        // path is NOT NULL but meaningless for a fileless book (empty string);
        // scan_key stays NULL so the reindex diff never sees this row.
        "INSERT INTO books
            (uuid, library_id, path, title, sort, author_sort, pubdate, has_cover,
             description, title_norm, author_norm, timestamp, last_modified)
         VALUES (?1, ?2, '', ?3, ?3, ?4, ?5, ?6, ?7, ?8, ?9,
                 strftime('%s','now'), strftime('%s','now'))
         RETURNING id",
    )
    .bind(&uuid)
    .bind(library_id)
    .bind(&book.title)
    .bind(&author_sort)
    .bind(&book.pubdate)
    .bind(has_cover)
    .bind(&book.description)
    .bind(normalize_title(&book.title))
    .bind(&author_norm)
    .fetch_one(&mut *tx)
    .await?;

    link_authors(&mut tx, book_id, &book.authors).await?;

    if let Some(isbn) = book.isbn.as_deref().filter(|s| !s.is_empty()) {
        sqlx::query(
            "INSERT OR IGNORE INTO book_identifiers (book_id, scheme, value)
             VALUES (?1, 'ISBN', ?2)",
        )
        .bind(book_id)
        .bind(isbn)
        .execute(&mut *tx)
        .await?;
    }

    // Index into `books_fts` through the single door so a physical-only book is
    // searchable like any other (title/authors/ISBN) — the write paths that hide
    // fileless books already include those with a physical copy.
    crate::sync::upsert_fts(&mut tx, book_id).await?;

    if let Some(cover) = &book.cover {
        write_cover_file(&uuid, &cover.mime, &cover.bytes)?;
    }

    tx.commit().await?;
    Ok(uuid)
}

/// Ensure the synthetic Physical scan root exists and return its id. Idempotent
/// — recreates the row if a prior orphan-prune swept it while no physical book
/// referenced it.
async fn ensure_physical_library(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
) -> Result<i64, sqlx::Error> {
    sqlx::query(
        "INSERT INTO scan_roots (path, display_name) VALUES (?1, 'Physical')
         ON CONFLICT(path) DO NOTHING",
    )
    .bind(PHYSICAL_LIBRARY_PATH)
    .execute(&mut **tx)
    .await?;
    sqlx::query_scalar::<_, i64>("SELECT id FROM scan_roots WHERE path = ?1")
        .bind(PHYSICAL_LIBRARY_PATH)
        .fetch_one(&mut **tx)
        .await
}

/// Resolve-or-insert every author and link them to the book in position order.
/// Fileless author lists are always small (1-3 names, already in memory), so
/// this batches the whole list into two statements instead of ~3 queries per
/// author — same insert-or-ignore semantics as a per-author loop, since
/// `books_authors_link`'s primary key is `(book, author)`: a name repeated in
/// `authors` still links only once, keeping its first position.
async fn link_authors(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    authors: &[String],
) -> Result<(), sqlx::Error> {
    if authors.is_empty() {
        return Ok(());
    }

    let author_rows = std::iter::repeat_n("(?)", authors.len())
        .collect::<Vec<_>>()
        .join(", ");
    let insert_sql = format!("INSERT OR IGNORE INTO authors (name) VALUES {author_rows}");
    let mut insert_q = sqlx::query(&insert_sql);
    for name in authors {
        insert_q = insert_q.bind(name);
    }
    insert_q.execute(&mut **tx).await?;

    let link_rows = std::iter::repeat_n("(?, ?)", authors.len())
        .collect::<Vec<_>>()
        .join(", ");
    let link_sql = format!(
        "INSERT OR IGNORE INTO books_authors_link (book, author, position)
         SELECT ?, a.id, v.column2
         FROM (VALUES {link_rows}) AS v
         JOIN authors a ON a.name = v.column1"
    );
    let mut link_q = sqlx::query(&link_sql).bind(book_id);
    for (position, name) in authors.iter().enumerate() {
        link_q = link_q.bind(name).bind(position as i64);
    }
    link_q.execute(&mut **tx).await?;
    Ok(())
}
