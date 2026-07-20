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

    for (position, name) in book.authors.iter().enumerate() {
        link_author(&mut tx, book_id, position as i64, name).await?;
    }

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

/// Resolve-or-insert an author by name and link it to the book at `position`.
async fn link_author(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    book_id: i64,
    position: i64,
    name: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO authors (name) VALUES (?1)")
        .bind(name)
        .execute(&mut **tx)
        .await?;
    let author_id: i64 = sqlx::query_scalar("SELECT id FROM authors WHERE name = ?1")
        .bind(name)
        .fetch_one(&mut **tx)
        .await?;
    sqlx::query(
        "INSERT OR IGNORE INTO books_authors_link (book, author, position)
         VALUES (?1, ?2, ?3)",
    )
    .bind(book_id)
    .bind(author_id)
    .bind(position)
    .execute(&mut **tx)
    .await?;
    Ok(())
}
