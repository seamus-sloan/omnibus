//! Normalized DB layer (server-only).
//!
//! The schema (see `../migrations/0002_normalized_schema.sql`) splits a
//! logical `books` row from one-or-more `book_files` rows per format, and
//! normalizes authors, series, tags, publishers, languages, and identifiers
//! into their own tables joined via m2m link tables. The filesystem remains
//! the source of truth — every row here is rebuildable by reindexing.
//!
//! The public API here preserves the shape older callers expect
//! (`replace_books`, `list_books`, `library_from_db`, `get_cover`,
//! `last_indexed_at`) so the indexer, rpc, and backend layers only need to
//! change at the edges. Internally, each of those functions drives the
//! normalized layout.

use std::path::{Path, PathBuf};

use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool, Transaction};

pub use omnibus_shared::Settings;
use omnibus_shared::{Contributor, EbookLibrary, EbookMetadata, Identifier};

/// Schema migrations embedded at compile time from `db/migrations/`.
/// Every schema change ships as a new numbered `.sql` file there; applied
/// versions are recorded in the `_sqlx_migrations` table.
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;

    MIGRATOR
        .run(&pool)
        .await
        .map_err(|e| sqlx::Error::Migrate(Box::new(e)))?;

    // SQLite does not enforce ON DELETE CASCADE unless foreign_keys is on
    // per-connection. Enable it here so that deleting a `books` row cascades
    // through every link table.
    sqlx::query("PRAGMA foreign_keys = ON")
        .execute(&pool)
        .await?;

    Ok(pool)
}

// -----------------------------------------------------------------------------
// app_state (counter) — unchanged from 0001 baseline.
// -----------------------------------------------------------------------------

pub async fn get_value(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let value = sqlx::query_scalar::<_, i64>("SELECT value FROM app_state WHERE id = 1")
        .fetch_one(pool)
        .await?;
    Ok(value)
}

pub async fn increment_value(pool: &SqlitePool) -> Result<i64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query("UPDATE app_state SET value = value + 1 WHERE id = 1")
        .execute(&mut *tx)
        .await?;
    let value = sqlx::query_scalar::<_, i64>("SELECT value FROM app_state WHERE id = 1")
        .fetch_one(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(value)
}

// -----------------------------------------------------------------------------
// Settings — KV keys remain `ebook_library_path` / `audiobook_library_path`.
// The indexer reconciles these into the `libraries` table on each reindex,
// so the settings UI can stay unchanged while the internal storage is
// normalized. F0.6 will replace this with a first-class libraries UI.
// -----------------------------------------------------------------------------

pub async fn get_settings(pool: &SqlitePool) -> Result<Settings, sqlx::Error> {
    let ebook_library_path = sqlx::query_scalar::<_, String>(
        "SELECT value FROM settings WHERE key = 'ebook_library_path'",
    )
    .fetch_optional(pool)
    .await?;
    let audiobook_library_path = sqlx::query_scalar::<_, String>(
        "SELECT value FROM settings WHERE key = 'audiobook_library_path'",
    )
    .fetch_optional(pool)
    .await?;
    Ok(Settings {
        ebook_library_path,
        audiobook_library_path,
    })
}

pub async fn set_settings(pool: &SqlitePool, settings: &Settings) -> Result<(), sqlx::Error> {
    upsert_or_clear(
        pool,
        "ebook_library_path",
        settings.ebook_library_path.as_deref(),
    )
    .await?;
    upsert_or_clear(
        pool,
        "audiobook_library_path",
        settings.audiobook_library_path.as_deref(),
    )
    .await?;
    Ok(())
}

async fn upsert_or_clear(
    pool: &SqlitePool,
    key: &str,
    value: Option<&str>,
) -> Result<(), sqlx::Error> {
    match value {
        Some(v) => {
            sqlx::query("INSERT OR REPLACE INTO settings (key, value) VALUES (?, ?)")
                .bind(key)
                .bind(v)
                .execute(pool)
                .await?;
        }
        None => {
            sqlx::query("DELETE FROM settings WHERE key = ?")
                .bind(key)
                .execute(pool)
                .await?;
        }
    }
    Ok(())
}

pub async fn seed_settings_from_env(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    let ebook_library_path = std::env::var("EBOOK_LIBRARY_PATH").ok();
    let audiobook_library_path = std::env::var("AUDIOBOOK_LIBRARY_PATH").ok();
    if ebook_library_path.is_some() || audiobook_library_path.is_some() {
        set_settings(
            pool,
            &Settings {
                ebook_library_path,
                audiobook_library_path,
            },
        )
        .await?;
    }
    Ok(())
}

// -----------------------------------------------------------------------------
// Libraries — one row per configured scan directory. `display_name` is
// derived from the directory basename for now; F0.6 will let users edit it.
// -----------------------------------------------------------------------------

async fn upsert_library(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    path: &str,
) -> Result<i64, sqlx::Error> {
    let display_name = Path::new(path)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or(path)
        .to_string();
    sqlx::query(
        "INSERT INTO libraries (path, display_name) VALUES (?, ?)
         ON CONFLICT(path) DO UPDATE SET display_name = excluded.display_name",
    )
    .bind(path)
    .bind(&display_name)
    .execute(&mut **tx)
    .await?;
    let id: i64 = sqlx::query_scalar("SELECT id FROM libraries WHERE path = ?")
        .bind(path)
        .fetch_one(&mut **tx)
        .await?;
    Ok(id)
}

/// Unix-seconds timestamp of the last successful index for `library_path`,
/// or `None` if the library has never been indexed (or doesn't exist in the
/// `libraries` table yet).
pub async fn last_indexed_at(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Option<i64>, sqlx::Error> {
    sqlx::query_scalar::<_, Option<i64>>("SELECT last_indexed FROM libraries WHERE path = ?")
        .bind(library_path)
        .fetch_optional(pool)
        .await
        .map(|opt| opt.flatten())
}

// -----------------------------------------------------------------------------
// Taxonomy resolve-or-insert helpers. Each returns the row id for the given
// (case-insensitive) name, inserting a row if one doesn't exist yet.
// -----------------------------------------------------------------------------

async fn resolve_or_insert_author(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    name: &str,
    sort: Option<&str>,
) -> Result<i64, sqlx::Error> {
    sqlx::query(
        "INSERT INTO authors (name, sort) VALUES (?, ?)
         ON CONFLICT(name) DO UPDATE SET sort = COALESCE(authors.sort, excluded.sort)",
    )
    .bind(name)
    .bind(sort)
    .execute(&mut **tx)
    .await?;
    sqlx::query_scalar("SELECT id FROM authors WHERE name = ?")
        .bind(name)
        .fetch_one(&mut **tx)
        .await
}

async fn resolve_or_insert_series(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    name: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO series (name) VALUES (?)")
        .bind(name)
        .execute(&mut **tx)
        .await?;
    sqlx::query_scalar("SELECT id FROM series WHERE name = ?")
        .bind(name)
        .fetch_one(&mut **tx)
        .await
}

async fn resolve_or_insert_tag(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    name: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO tags (name) VALUES (?)")
        .bind(name)
        .execute(&mut **tx)
        .await?;
    sqlx::query_scalar("SELECT id FROM tags WHERE name = ?")
        .bind(name)
        .fetch_one(&mut **tx)
        .await
}

async fn resolve_or_insert_publisher(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    name: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO publishers (name) VALUES (?)")
        .bind(name)
        .execute(&mut **tx)
        .await?;
    sqlx::query_scalar("SELECT id FROM publishers WHERE name = ?")
        .bind(name)
        .fetch_one(&mut **tx)
        .await
}

async fn resolve_or_insert_language(
    tx: &mut Transaction<'_, sqlx::Sqlite>,
    code: &str,
) -> Result<i64, sqlx::Error> {
    sqlx::query("INSERT OR IGNORE INTO languages (code) VALUES (?)")
        .bind(code)
        .execute(&mut **tx)
        .await?;
    sqlx::query_scalar("SELECT id FROM languages WHERE code = ?")
        .bind(code)
        .fetch_one(&mut **tx)
        .await
}

// -----------------------------------------------------------------------------
// Covers on filesystem.
//
// Covers are stored under `<OMNIBUS_COVERS_DIR>/<uuid>.<ext>` so a backup of
// the SQLite DB stays small and covers can be regenerated independently by
// reindexing. `has_cover` on the `books` row tracks whether a file should
// exist; a missing file on disk is treated as "no cover" (404), not an
// error.
// -----------------------------------------------------------------------------

/// Root directory for cover files. Override with `OMNIBUS_COVERS_DIR`.
pub fn covers_dir() -> PathBuf {
    std::env::var("OMNIBUS_COVERS_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./covers"))
}

fn mime_to_ext(mime: &str) -> &'static str {
    match mime.to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/png" => "png",
        "image/gif" => "gif",
        "image/webp" => "webp",
        "image/svg+xml" => "svg",
        _ => "bin",
    }
}

fn ext_to_mime(ext: &str) -> String {
    match ext.to_ascii_lowercase().as_str() {
        "jpg" | "jpeg" => "image/jpeg".to_string(),
        "png" => "image/png".to_string(),
        "gif" => "image/gif".to_string(),
        "webp" => "image/webp".to_string(),
        "svg" => "image/svg+xml".to_string(),
        _ => "application/octet-stream".to_string(),
    }
}

fn cover_path_for(uuid: &str, ext: &str) -> PathBuf {
    covers_dir().join(format!("{uuid}.{ext}"))
}

fn write_cover_file(uuid: &str, mime: &str, bytes: &[u8]) -> std::io::Result<()> {
    let dir = covers_dir();
    std::fs::create_dir_all(&dir)?;
    let ext = mime_to_ext(mime);
    std::fs::write(cover_path_for(uuid, ext), bytes)
}

fn find_cover_file(uuid: &str) -> Option<(String, Vec<u8>)> {
    // Try common extensions in the order covers are most likely to be
    // written. Fall back to a directory scan for `<uuid>.*` if none match,
    // so migrations that introduce new extensions don't require a code
    // change here.
    for ext in ["jpg", "png", "webp", "gif", "svg", "bin"] {
        let p = cover_path_for(uuid, ext);
        if let Ok(bytes) = std::fs::read(&p) {
            return Some((ext_to_mime(ext), bytes));
        }
    }
    // Fallback scan.
    let dir = covers_dir();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let name_str = name.to_string_lossy();
            if let Some(stem) = name_str.strip_suffix(&format!(".{uuid}")) {
                // uuid is the prefix, not suffix; keep falling through
                let _ = stem;
            }
            if let Some(dot) = name_str.rfind('.') {
                let (stem, ext) = name_str.split_at(dot);
                if stem == uuid {
                    if let Ok(bytes) = std::fs::read(entry.path()) {
                        return Some((ext_to_mime(&ext[1..]), bytes));
                    }
                }
            }
        }
    }
    None
}

fn delete_cover_files_for(uuids: &[String]) {
    for uuid in uuids {
        for ext in ["jpg", "png", "webp", "gif", "svg", "bin"] {
            let _ = std::fs::remove_file(cover_path_for(uuid, ext));
        }
    }
}

/// Load a book's cover image bytes + mime type from disk. The `id` parameter
/// is the `books.id` primary key (so the `/api/covers/:id` URL shape stays
/// stable); internally we look up the book's `uuid` and read the file.
pub async fn get_cover(
    pool: &SqlitePool,
    book_id: i64,
) -> Result<Option<(String, Vec<u8>)>, sqlx::Error> {
    let row: Option<(String, i64)> =
        sqlx::query_as("SELECT uuid, has_cover FROM books WHERE id = ?")
            .bind(book_id)
            .fetch_optional(pool)
            .await?;
    match row {
        Some((uuid, has_cover)) if has_cover != 0 => Ok(find_cover_file(&uuid)),
        _ => Ok(None),
    }
}

// -----------------------------------------------------------------------------
// Indexer write path.
// -----------------------------------------------------------------------------

/// Atomically replace every book under `library_path` with `books` and stamp
/// the last-indexed time. Upserts a matching `libraries` row if one doesn't
/// exist yet. The cascade from `books` → link tables + `book_files` runs
/// automatically thanks to the `PRAGMA foreign_keys = ON` set in `init_db`.
pub async fn replace_books(
    pool: &SqlitePool,
    library_path: &str,
    books: Vec<crate::ebook::IndexedBook>,
) -> Result<(), sqlx::Error> {
    // Collect uuids of books we're about to delete so we can clean up their
    // cover files on disk. Happens before the transaction so a mid-txn
    // failure doesn't leave orphaned files, at the cost of a tiny window
    // where disk + DB disagree on rollback (acceptable; covers are a
    // rebuildable cache).
    let old_uuids: Vec<String> = sqlx::query_scalar(
        "SELECT b.uuid FROM books b
         JOIN libraries l ON l.id = b.library_id
         WHERE l.path = ?",
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;

    let mut tx = pool.begin().await?;
    let library_id = upsert_library(&mut tx, library_path).await?;

    // `books_fts` is a standalone FTS5 table with no FK/cascade to `books`,
    // so we must clear its rows explicitly before the cascade delete below.
    // Scoped to this library via the books.id → books_fts.rowid mapping.
    sqlx::query(
        "DELETE FROM books_fts WHERE rowid IN
            (SELECT id FROM books WHERE library_id = ?)",
    )
    .bind(library_id)
    .execute(&mut *tx)
    .await?;

    sqlx::query("DELETE FROM books WHERE library_id = ?")
        .bind(library_id)
        .execute(&mut *tx)
        .await?;

    let mut new_covers: Vec<(String, String, Vec<u8>)> = Vec::new();

    for b in books {
        let m = &b.metadata;
        let uuid = stable_uuid(library_path, &m.filename);
        let (book_path, file_stem, file_ext) = split_filename(&m.filename);
        let title = m.title.clone().unwrap_or_else(|| m.filename.clone());
        let series_index_num = m.series_index.as_deref().and_then(parse_series_index);
        let author_sort = m
            .creators
            .first()
            .and_then(|c| c.file_as.clone())
            .or_else(|| m.creators.first().map(|c| c.name.clone()));
        let first_isbn = m
            .identifiers
            .iter()
            .find(|id| {
                id.scheme
                    .as_deref()
                    .is_some_and(|s| s.eq_ignore_ascii_case("isbn"))
            })
            .map(|id| id.value.clone());
        let has_cover = if b.cover.is_some() { 1 } else { 0 };

        let book_id = sqlx::query_scalar::<_, i64>(
            "INSERT INTO books
                (uuid, library_id, path, title, sort, author_sort, series_index,
                 pubdate, has_cover, description, isbn)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             RETURNING id",
        )
        .bind(&uuid)
        .bind(library_id)
        .bind(&book_path)
        .bind(&title)
        .bind(&title)
        .bind(&author_sort)
        .bind(series_index_num)
        .bind(&m.published)
        .bind(has_cover)
        .bind(&m.description)
        .bind(&first_isbn)
        .fetch_one(&mut *tx)
        .await?;

        let size_bytes = 0i64;
        let mtime = m.modified.clone().unwrap_or_default();
        sqlx::query(
            "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime)
             VALUES (?, ?, ?, ?, ?)",
        )
        .bind(book_id)
        .bind(&file_ext)
        .bind(&file_stem)
        .bind(size_bytes)
        .bind(&mtime)
        .execute(&mut *tx)
        .await?;

        // Authors + contributors both land in `authors` — role/file_as are
        // flattened. The first creator gets position 0.
        for (pos, c) in m.creators.iter().enumerate() {
            let author_id =
                resolve_or_insert_author(&mut tx, &c.name, c.file_as.as_deref()).await?;
            sqlx::query(
                "INSERT OR IGNORE INTO books_authors_link (book, author, position)
                 VALUES (?, ?, ?)",
            )
            .bind(book_id)
            .bind(author_id)
            .bind(pos as i64)
            .execute(&mut *tx)
            .await?;
        }
        let author_count = m.creators.len();
        for (i, c) in m.contributors.iter().enumerate() {
            let author_id =
                resolve_or_insert_author(&mut tx, &c.name, c.file_as.as_deref()).await?;
            sqlx::query(
                "INSERT OR IGNORE INTO books_authors_link (book, author, position)
                 VALUES (?, ?, ?)",
            )
            .bind(book_id)
            .bind(author_id)
            .bind((author_count + i) as i64)
            .execute(&mut *tx)
            .await?;
        }

        if let Some(series_name) = m.series.as_deref().filter(|s| !s.is_empty()) {
            let series_id = resolve_or_insert_series(&mut tx, series_name).await?;
            sqlx::query("INSERT OR IGNORE INTO books_series_link (book, series) VALUES (?, ?)")
                .bind(book_id)
                .bind(series_id)
                .execute(&mut *tx)
                .await?;
        }

        for subject in &m.subjects {
            if subject.is_empty() {
                continue;
            }
            let tag_id = resolve_or_insert_tag(&mut tx, subject).await?;
            sqlx::query("INSERT OR IGNORE INTO books_tags_link (book, tag) VALUES (?, ?)")
                .bind(book_id)
                .bind(tag_id)
                .execute(&mut *tx)
                .await?;
        }

        if let Some(pub_name) = m.publisher.as_deref().filter(|s| !s.is_empty()) {
            let pub_id = resolve_or_insert_publisher(&mut tx, pub_name).await?;
            sqlx::query(
                "INSERT OR IGNORE INTO books_publishers_link (book, publisher) VALUES (?, ?)",
            )
            .bind(book_id)
            .bind(pub_id)
            .execute(&mut *tx)
            .await?;
        }

        if let Some(lang_code) = m.language.as_deref().filter(|s| !s.is_empty()) {
            let lang_id = resolve_or_insert_language(&mut tx, lang_code).await?;
            sqlx::query(
                "INSERT OR IGNORE INTO books_languages_link (book, language) VALUES (?, ?)",
            )
            .bind(book_id)
            .bind(lang_id)
            .execute(&mut *tx)
            .await?;
        }

        for ident in &m.identifiers {
            if ident.value.is_empty() {
                continue;
            }
            let scheme = ident
                .scheme
                .clone()
                .unwrap_or_else(|| "unknown".to_string());
            sqlx::query(
                "INSERT OR REPLACE INTO book_identifiers (book_id, scheme, value)
                 VALUES (?, ?, ?)",
            )
            .bind(book_id)
            .bind(&scheme)
            .bind(&ident.value)
            .execute(&mut *tx)
            .await?;
        }

        // FTS row. Written inline so the bulk reindex doesn't need a trigger
        // fan-out across six tables. Keeps the denormalized row in lock-step
        // with the book's canonical inserts above.
        let authors_text = join_names(
            m.creators
                .iter()
                .chain(m.contributors.iter())
                .map(|c| c.name.as_str()),
        );
        let series_text = m.series.clone().unwrap_or_default();
        let tags_text = join_names(m.subjects.iter().map(String::as_str));
        sqlx::query(
            "INSERT INTO books_fts(rowid, title, authors, series, tags, description, isbn)
             VALUES (?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(book_id)
        .bind(&title)
        .bind(&authors_text)
        .bind(&series_text)
        .bind(&tags_text)
        .bind(m.description.as_deref().unwrap_or(""))
        .bind(first_isbn.as_deref().unwrap_or(""))
        .execute(&mut *tx)
        .await?;

        if let Some((mime, bytes)) = b.cover {
            new_covers.push((uuid, mime, bytes));
        }
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

    // DB commit succeeded — now reconcile the covers directory. Delete the
    // files for every book that was replaced, then write out the new covers.
    delete_cover_files_for(&old_uuids);
    for (uuid, mime, bytes) in new_covers {
        if let Err(e) = write_cover_file(&uuid, &mime, &bytes) {
            eprintln!("replace_books: failed to write cover for {uuid}: {e}");
        }
    }

    Ok(())
}

// -----------------------------------------------------------------------------
// Read path — list_books + library_from_db produce `EbookMetadata` shapes so
// the wire API can remain unchanged while the underlying storage is
// normalized.
// -----------------------------------------------------------------------------

/// Return every book indexed under `library_path`. Multi-valued fields are
/// materialized via follow-up queries rather than `GROUP_CONCAT` so the
/// shapes round-trip cleanly through JSON and we don't have to parse
/// delimiter-escaped strings.
pub async fn list_books(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Vec<EbookMetadata>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT b.id, b.uuid, bf.filename AS file_stem, bf.format AS file_format,
               b.title, b.description, b.series_index, b.has_cover,
               b.pubdate, b.last_modified, b.isbn,
               pub.name AS publisher_name, lang.code AS language_code,
               s.name AS series_name
        FROM books b
        JOIN libraries l ON l.id = b.library_id
        LEFT JOIN book_files bf ON bf.book_id = b.id
        LEFT JOIN books_publishers_link bpl ON bpl.book = b.id
        LEFT JOIN publishers pub ON pub.id = bpl.publisher
        LEFT JOIN books_languages_link bll ON bll.book = b.id
        LEFT JOIN languages lang ON lang.id = bll.language
        LEFT JOIN books_series_link bsl ON bsl.book = b.id
        LEFT JOIN series s ON s.id = bsl.series
        WHERE l.path = ?
        ORDER BY b.sort, b.id
        "#,
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id: i64 = r.get("id");
        let has_cover: i64 = r.get("has_cover");
        let file_stem: Option<String> = r.get("file_stem");
        let file_format: Option<String> = r.get("file_format");
        let filename = match (file_stem, file_format) {
            (Some(stem), Some(fmt)) => format!("{stem}.{}", fmt.to_ascii_lowercase()),
            _ => String::new(),
        };
        let series_index: Option<f64> = r.get("series_index");

        let creators = load_creators(pool, id).await?;
        let subjects = load_subjects(pool, id).await?;
        let identifiers = load_identifiers(pool, id).await?;

        out.push(EbookMetadata {
            id,
            filename,
            title: r.get("title"),
            description: r.get("description"),
            publisher: r.get("publisher_name"),
            published: r.get("pubdate"),
            modified: r.get("last_modified"),
            language: r.get("language_code"),
            rights: None,
            source: None,
            coverage: None,
            dc_type: None,
            dc_format: None,
            relation: None,
            creators,
            contributors: vec![],
            subjects,
            identifiers,
            series: r.get("series_name"),
            series_index: series_index.map(format_series_index),
            epub_version: None,
            unique_identifier: Some(r.get::<String, _>("uuid")),
            resource_count: 0,
            spine_count: 0,
            toc_count: 0,
            cover_url: (has_cover != 0).then(|| format!("/api/covers/{id}")),
            error: None,
        });
    }
    Ok(out)
}

async fn load_creators(pool: &SqlitePool, book_id: i64) -> Result<Vec<Contributor>, sqlx::Error> {
    let rows = sqlx::query(
        "SELECT a.name, a.sort FROM books_authors_link bal
         JOIN authors a ON a.id = bal.author
         WHERE bal.book = ?
         ORDER BY bal.position",
    )
    .bind(book_id)
    .fetch_all(pool)
    .await?;
    Ok(rows
        .into_iter()
        .map(|r| Contributor {
            name: r.get("name"),
            role: None,
            file_as: r.get("sort"),
        })
        .collect())
}

async fn load_subjects(pool: &SqlitePool, book_id: i64) -> Result<Vec<String>, sqlx::Error> {
    sqlx::query_scalar(
        "SELECT t.name FROM books_tags_link btl
         JOIN tags t ON t.id = btl.tag
         WHERE btl.book = ?
         ORDER BY t.name",
    )
    .bind(book_id)
    .fetch_all(pool)
    .await
}

async fn load_identifiers(pool: &SqlitePool, book_id: i64) -> Result<Vec<Identifier>, sqlx::Error> {
    let rows = sqlx::query("SELECT scheme, value FROM book_identifiers WHERE book_id = ?")
        .bind(book_id)
        .fetch_all(pool)
        .await?;
    Ok(rows
        .into_iter()
        .map(|r| Identifier {
            value: r.get("value"),
            scheme: Some(r.get("scheme")),
        })
        .collect())
}

/// Full-text search across `books_fts`. Returns hydrated `EbookMetadata`
/// ordered by bm25 rank (best first). Matching is scoped to
/// `title/authors/series` via a column filter so that short prefix queries
/// don't surface spurious hits on generic `tags` values (e.g. typing "Dr"
/// matching books tagged "Drama"). Ranking weights favour title matches:
/// `bm25(books_fts, 10.0, 4.0, 3.0, 1.0, 1.0, 1.0)` — unused columns keep
/// neutral weights since the column filter prevents them from contributing.
///
/// `q` is sanitized via [`sanitize_fts_query`] before reaching `MATCH`, so
/// arbitrary user input is safe to pass through. Returns an empty vec when
/// the sanitized query is empty.
pub async fn search_books(
    pool: &SqlitePool,
    library_path: &str,
    q: &str,
) -> Result<Vec<EbookMetadata>, sqlx::Error> {
    let Some(sanitized) = sanitize_fts_query(q) else {
        return Ok(Vec::new());
    };
    // FTS5 column filter. Scoping to title/authors/series keeps short
    // queries like "Dr" from matching generic `tags` values ("Drama") or
    // `description` contents. ISBN, tags, description remain in the index
    // so they can be re-enabled without a migration.
    let match_expr = format!("{{title authors series}} : ({sanitized})");

    let rows = sqlx::query(
        r#"
        SELECT b.id, b.uuid, bf.filename AS file_stem, bf.format AS file_format,
               b.title, b.description, b.series_index, b.has_cover,
               b.pubdate, b.last_modified, b.isbn,
               pub.name AS publisher_name, lang.code AS language_code,
               s.name AS series_name
        FROM books_fts
        JOIN books b ON b.id = books_fts.rowid
        JOIN libraries l ON l.id = b.library_id
        LEFT JOIN book_files bf ON bf.book_id = b.id
        LEFT JOIN books_publishers_link bpl ON bpl.book = b.id
        LEFT JOIN publishers pub ON pub.id = bpl.publisher
        LEFT JOIN books_languages_link bll ON bll.book = b.id
        LEFT JOIN languages lang ON lang.id = bll.language
        LEFT JOIN books_series_link bsl ON bsl.book = b.id
        LEFT JOIN series s ON s.id = bsl.series
        WHERE books_fts MATCH ? AND l.path = ?
        ORDER BY bm25(books_fts, 10.0, 4.0, 3.0, 1.0, 1.0, 1.0), b.sort, b.id
        "#,
    )
    .bind(&match_expr)
    .bind(library_path)
    .fetch_all(pool)
    .await?;

    let mut out = Vec::with_capacity(rows.len());
    for r in rows {
        let id: i64 = r.get("id");
        let has_cover: i64 = r.get("has_cover");
        let file_stem: Option<String> = r.get("file_stem");
        let file_format: Option<String> = r.get("file_format");
        let filename = match (file_stem, file_format) {
            (Some(stem), Some(fmt)) => format!("{stem}.{}", fmt.to_ascii_lowercase()),
            _ => String::new(),
        };
        let series_index: Option<f64> = r.get("series_index");

        let creators = load_creators(pool, id).await?;
        let subjects = load_subjects(pool, id).await?;
        let identifiers = load_identifiers(pool, id).await?;

        out.push(EbookMetadata {
            id,
            filename,
            title: r.get("title"),
            description: r.get("description"),
            publisher: r.get("publisher_name"),
            published: r.get("pubdate"),
            modified: r.get("last_modified"),
            language: r.get("language_code"),
            rights: None,
            source: None,
            coverage: None,
            dc_type: None,
            dc_format: None,
            relation: None,
            creators,
            contributors: vec![],
            subjects,
            identifiers,
            series: r.get("series_name"),
            series_index: series_index.map(format_series_index),
            epub_version: None,
            unique_identifier: Some(r.get::<String, _>("uuid")),
            resource_count: 0,
            spine_count: 0,
            toc_count: 0,
            cover_url: (has_cover != 0).then(|| format!("/api/covers/{id}")),
            error: None,
        });
    }
    Ok(out)
}

/// Build an `EbookLibrary` from whatever is currently in the DB for
/// `library_path`. Returns an empty library (no error, no books) if the path
/// is `None`.
pub async fn library_from_db(
    pool: &SqlitePool,
    library_path: Option<&str>,
) -> Result<EbookLibrary, sqlx::Error> {
    let Some(path) = library_path else {
        return Ok(EbookLibrary::default());
    };
    let books = list_books(pool, path).await?;
    Ok(EbookLibrary {
        path: Some(path.to_string()),
        books,
        error: None,
    })
}

// -----------------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------------

/// Deterministic UUIDv5-shaped string derived from (library_path, filename)
/// so reindexing the same file produces the same uuid. Keeps
/// `/api/covers/:id` URLs stable across reindex cycles even as the primary
/// `books.id` renumbers.
fn stable_uuid(library_path: &str, filename: &str) -> String {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    library_path.hash(&mut h);
    filename.hash(&mut h);
    let a = h.finish();
    let mut h2 = DefaultHasher::new();
    (library_path, filename, a).hash(&mut h2);
    let b = h2.finish();
    format!(
        "{:08x}-{:04x}-{:04x}-{:04x}-{:012x}",
        (a >> 32) as u32,
        (a >> 16) as u16,
        a as u16,
        (b >> 48) as u16,
        b & 0x0000_ffff_ffff_ffff,
    )
}

/// Split `dir/sub/name.epub` into (`dir/sub`, `name`, `EPUB`). If no dir,
/// the path portion is empty. Extension is uppercased per Calibre convention.
fn split_filename(filename: &str) -> (String, String, String) {
    let path = Path::new(filename);
    let parent = path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_else(|| filename.to_string());
    let ext = path
        .extension()
        .map(|s| s.to_string_lossy().to_ascii_uppercase())
        .unwrap_or_else(|| "UNKNOWN".to_string());
    (parent, stem, ext)
}

fn parse_series_index(s: &str) -> Option<f64> {
    s.trim().parse::<f64>().ok()
}

/// Join an iterator of names into a single whitespace-separated string for
/// the FTS `authors` / `tags` columns. Empty inputs collapse to "".
fn join_names<'a, I: IntoIterator<Item = &'a str>>(iter: I) -> String {
    let mut out = String::new();
    for name in iter {
        if name.is_empty() {
            continue;
        }
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(name);
    }
    out
}

/// Wrap each whitespace-separated token in double-quotes and append `*` to
/// the last one for prefix matching. This lets the user type plain words
/// (including FTS5-reserved tokens like `AND`/`NOT` or hyphenated ISBNs)
/// without triggering a `MATCH` parse error, and gives type-ahead cheaply.
///
/// Returns `None` when the sanitized query is empty — callers should treat
/// that as "don't run the query" rather than passing an empty MATCH.
pub fn sanitize_fts_query(raw: &str) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for token in raw.split_whitespace() {
        // Double quotes inside a token would terminate the quoted phrase.
        // FTS5's quoted phrase escape is `""`, so double every quote.
        let escaped = token.replace('"', "\"\"");
        if escaped.is_empty() {
            continue;
        }
        parts.push(format!("\"{escaped}\""));
    }
    if parts.is_empty() {
        return None;
    }
    let last = parts.len() - 1;
    parts[last].push('*');
    Some(parts.join(" "))
}

fn format_series_index(v: f64) -> String {
    if (v - v.trunc()).abs() < f64::EPSILON {
        format!("{}", v.trunc() as i64)
    } else {
        format!("{v}")
    }
}

// -----------------------------------------------------------------------------
// Tests
// -----------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Per-test covers dir so parallel tests don't stomp each other. Sets
    /// OMNIBUS_COVERS_DIR and returns the path; the caller drops the guard
    /// at end-of-test to clean up.
    // OMNIBUS_COVERS_DIR is a process-global env var, so tests that touch it
    // must serialize. A single Mutex held for the duration of each test keeps
    // parallel `cargo test` runs from stomping on each other's covers dir.
    static COVERS_ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    struct CoversTempDir {
        path: PathBuf,
        prev: Option<String>,
        _guard: std::sync::MutexGuard<'static, ()>,
    }

    impl CoversTempDir {
        fn new(tag: &str) -> Self {
            let guard = COVERS_ENV_LOCK
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let pid = std::process::id();
            let seq = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0);
            let path = std::env::temp_dir().join(format!("omnibus_covers_{tag}_{pid}_{seq}"));
            let _ = std::fs::remove_dir_all(&path);
            let prev = std::env::var("OMNIBUS_COVERS_DIR").ok();
            std::env::set_var("OMNIBUS_COVERS_DIR", &path);
            Self {
                path,
                prev,
                _guard: guard,
            }
        }
    }

    impl Drop for CoversTempDir {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.path);
            match self.prev.take() {
                Some(v) => std::env::set_var("OMNIBUS_COVERS_DIR", v),
                None => std::env::remove_var("OMNIBUS_COVERS_DIR"),
            }
        }
    }

    #[tokio::test]
    async fn migrator_records_applied_versions() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let versions: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .expect("_sqlx_migrations should exist after init_db");
        assert!(
            versions.contains(&1),
            "baseline migration 0001 should be recorded, got {versions:?}"
        );
        assert!(
            versions.contains(&2),
            "normalized migration 0002 should be recorded, got {versions:?}"
        );
        assert!(
            versions.contains(&3),
            "legacy-drop migration 0003 should be recorded, got {versions:?}"
        );
    }

    #[tokio::test]
    async fn migrator_is_idempotent_on_rerun() {
        let tmp = std::env::temp_dir().join(format!(
            "omnibus-migrate-{}-{}.db",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_file(&tmp);
        let url = format!("sqlite://{}?mode=rwc", tmp.display());

        let pool1 = init_db(&url).await.expect("first init");
        drop(pool1);
        let pool2 = init_db(&url).await.expect("second init");

        let by_version: Vec<(i64, i64)> =
            sqlx::query_as("SELECT version, COUNT(*) FROM _sqlx_migrations GROUP BY version")
                .fetch_all(&pool2)
                .await
                .unwrap();
        for (_, count) in by_version {
            assert_eq!(count, 1, "every migration recorded exactly once");
        }

        drop(pool2);
        let _ = std::fs::remove_file(&tmp);
    }

    #[tokio::test]
    async fn initializes_and_seeds_default_value() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let value = get_value(&pool).await.expect("seeded value should exist");
        assert_eq!(value, 0);
    }

    #[tokio::test]
    async fn increments_value_persistently() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let value = increment_value(&pool).await.unwrap();
        assert_eq!(value, 1);
        let value = get_value(&pool).await.unwrap();
        assert_eq!(value, 1);
    }

    #[tokio::test]
    async fn get_settings_returns_none_for_empty_db() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let settings = get_settings(&pool).await.unwrap();
        assert_eq!(settings.ebook_library_path, None);
        assert_eq!(settings.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn set_and_get_settings_roundtrips() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let input = Settings {
            ebook_library_path: Some("/books/ebooks".into()),
            audiobook_library_path: Some("/books/audio".into()),
        };
        set_settings(&pool, &input).await.unwrap();
        assert_eq!(get_settings(&pool).await.unwrap(), input);
    }

    #[tokio::test]
    async fn set_settings_updates_existing_values() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/old".into()),
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/new".into()),
                audiobook_library_path: Some("/audio".into()),
            },
        )
        .await
        .unwrap();
        let result = get_settings(&pool).await.unwrap();
        assert_eq!(result.ebook_library_path, Some("/new".into()));
        assert_eq!(result.audiobook_library_path, Some("/audio".into()));
    }

    #[tokio::test]
    async fn set_settings_none_clears_existing_value() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/books".into()),
                audiobook_library_path: Some("/audio".into()),
            },
        )
        .await
        .unwrap();
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: None,
                audiobook_library_path: None,
            },
        )
        .await
        .unwrap();
        let result = get_settings(&pool).await.unwrap();
        assert_eq!(result.ebook_library_path, None);
        assert_eq!(result.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn seed_settings_from_env_writes_env_vars_to_db() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        std::env::set_var("EBOOK_LIBRARY_PATH", "/env/books");
        std::env::set_var("AUDIOBOOK_LIBRARY_PATH", "/env/audio");
        seed_settings_from_env(&pool).await.unwrap();
        std::env::remove_var("EBOOK_LIBRARY_PATH");
        std::env::remove_var("AUDIOBOOK_LIBRARY_PATH");
        let result = get_settings(&pool).await.unwrap();
        assert_eq!(result.ebook_library_path, Some("/env/books".into()));
        assert_eq!(result.audiobook_library_path, Some("/env/audio".into()));
    }

    #[tokio::test]
    async fn seed_settings_from_env_is_noop_when_vars_unset() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        std::env::remove_var("EBOOK_LIBRARY_PATH");
        std::env::remove_var("AUDIOBOOK_LIBRARY_PATH");
        seed_settings_from_env(&pool).await.unwrap();
        let result = get_settings(&pool).await.unwrap();
        assert_eq!(result.ebook_library_path, None);
        assert_eq!(result.audiobook_library_path, None);
    }

    use crate::ebook::IndexedBook;

    fn indexed(
        filename: &str,
        title: Option<&str>,
        authors: &[&str],
        subjects: &[&str],
        series: Option<(&str, &str)>,
        cover: Option<(&str, &[u8])>,
    ) -> IndexedBook {
        IndexedBook {
            metadata: EbookMetadata {
                filename: filename.into(),
                title: title.map(Into::into),
                creators: authors
                    .iter()
                    .map(|a| Contributor {
                        name: (*a).into(),
                        ..Default::default()
                    })
                    .collect(),
                subjects: subjects.iter().map(|s| (*s).to_string()).collect(),
                series: series.map(|(n, _)| n.into()),
                series_index: series.map(|(_, i)| i.into()),
                ..Default::default()
            },
            cover: cover.map(|(m, b)| (m.into(), b.to_vec())),
        }
    }

    #[tokio::test]
    async fn replace_books_inserts_metadata_and_covers() {
        let _covers = CoversTempDir::new("insert");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("A"),
                    &["Author A"],
                    &["fiction"],
                    Some(("Saga", "1")),
                    Some(("image/jpeg", b"BYTES")),
                ),
                indexed("b.epub", Some("B"), &["Author B"], &[], None, None),
            ],
        )
        .await
        .expect("replace should succeed");

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 2);

        let a = books
            .iter()
            .find(|b| b.title.as_deref() == Some("A"))
            .unwrap();
        let b = books
            .iter()
            .find(|b| b.title.as_deref() == Some("B"))
            .unwrap();

        assert_eq!(a.filename, "a.epub");
        assert_eq!(b.filename, "b.epub");
        assert_eq!(a.creators.len(), 1);
        assert_eq!(a.creators[0].name, "Author A");
        assert_eq!(a.subjects, vec!["fiction".to_string()]);
        assert_eq!(a.series.as_deref(), Some("Saga"));
        assert_eq!(a.series_index.as_deref(), Some("1"));

        assert_eq!(
            a.cover_url.as_deref(),
            Some(format!("/api/covers/{}", a.id).as_str())
        );
        assert_eq!(b.cover_url, None);

        let cover = get_cover(&pool, a.id).await.unwrap();
        assert_eq!(cover, Some(("image/jpeg".into(), b"BYTES".to_vec())));
        assert!(get_cover(&pool, b.id).await.unwrap().is_none());

        assert!(last_indexed_at(&pool, "/lib").await.unwrap().is_some());
    }

    #[tokio::test]
    async fn reindex_replaces_library_atomically() {
        let _covers = CoversTempDir::new("atomic");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Author A"],
                &["fiction"],
                None,
                Some(("image/jpeg", b"OLD")),
            )],
        )
        .await
        .unwrap();

        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["Author A"],
                &["fiction"],
                None,
                Some(("image/jpeg", b"NEW")),
            )],
        )
        .await
        .unwrap();

        // No orphan rows in any link table for books that no longer exist.
        for table in [
            "books_authors_link",
            "books_tags_link",
            "books_series_link",
            "books_publishers_link",
            "books_languages_link",
        ] {
            let orphan: i64 = sqlx::query_scalar(&format!(
                "SELECT COUNT(*) FROM {table} WHERE book NOT IN (SELECT id FROM books)"
            ))
            .fetch_one(&pool)
            .await
            .unwrap();
            assert_eq!(orphan, 0, "{table} should have no orphans");
        }
        let orphan_files: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM book_files WHERE book_id NOT IN (SELECT id FROM books)",
        )
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(orphan_files, 0);

        let books = list_books(&pool, "/lib").await.unwrap();
        assert_eq!(books.len(), 1);
        let cover = get_cover(&pool, books[0].id).await.unwrap();
        assert_eq!(cover, Some(("image/jpeg".into(), b"NEW".to_vec())));
    }

    #[tokio::test]
    async fn author_dedupes_across_books_case_insensitive() {
        let _covers = CoversTempDir::new("dedupe");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None),
                indexed("b.epub", Some("B"), &["tolkien"], &[], None, None),
            ],
        )
        .await
        .unwrap();
        let n: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM authors")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(n, 1, "NOCASE unique should collapse Tolkien/tolkien");
    }

    #[tokio::test]
    async fn series_index_sorts_numerically() {
        // Regression guard against reintroducing Calibre's TEXT series_index:
        // 10 must sort after 2, not before.
        let _covers = CoversTempDir::new("series");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("b.epub", Some("B"), &["A"], &[], Some(("S", "10")), None),
                indexed("a.epub", Some("A"), &["A"], &[], Some(("S", "2")), None),
            ],
        )
        .await
        .unwrap();
        let indices: Vec<f64> =
            sqlx::query_scalar("SELECT series_index FROM books ORDER BY series_index")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(indices, vec![2.0, 10.0]);
    }

    #[tokio::test]
    async fn cover_returns_none_when_file_missing() {
        let _covers = CoversTempDir::new("missing");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("A"),
                &["A"],
                &[],
                None,
                Some(("image/jpeg", b"BYTES")),
            )],
        )
        .await
        .unwrap();
        let books = list_books(&pool, "/lib").await.unwrap();
        let uuid: String = sqlx::query_scalar("SELECT uuid FROM books WHERE id = ?")
            .bind(books[0].id)
            .fetch_one(&pool)
            .await
            .unwrap();
        // Remove the file out from under the DB — get_cover should report
        // None, not error.
        let _ = std::fs::remove_file(cover_path_for(&uuid, "jpg"));
        assert!(get_cover(&pool, books[0].id).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn library_from_db_returns_empty_for_none_path() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let lib = library_from_db(&pool, None).await.unwrap();
        assert!(lib.path.is_none());
        assert!(lib.books.is_empty());
        assert!(lib.error.is_none());
    }

    // ---------- FTS5 (F0.4) ----------

    #[test]
    fn sanitize_fts_query_quotes_tokens_and_prefixes_last() {
        assert_eq!(
            sanitize_fts_query("harry pott").as_deref(),
            Some("\"harry\" \"pott\"*")
        );
    }

    #[test]
    fn sanitize_fts_query_escapes_embedded_double_quotes() {
        assert_eq!(
            sanitize_fts_query("say \"hi").as_deref(),
            Some("\"say\" \"\"\"hi\"*")
        );
    }

    #[test]
    fn sanitize_fts_query_returns_none_for_empty_and_whitespace() {
        assert!(sanitize_fts_query("").is_none());
        assert!(sanitize_fts_query("   \t  ").is_none());
    }

    #[test]
    fn sanitize_fts_query_treats_operators_as_literals() {
        // Bare `AND` / `NOT` would otherwise be parsed as FTS5 operators and
        // could throw. Quoting makes them into literal tokens.
        let out = sanitize_fts_query("AND NOT OR").expect("non-empty");
        assert!(out.contains("\"AND\""));
        assert!(out.contains("\"NOT\""));
        assert!(out.contains("\"OR\"*"));
    }

    #[test]
    fn sanitize_fts_query_keeps_hyphenated_isbn_as_single_token() {
        let out = sanitize_fts_query("978-0-123456-78-9").expect("non-empty");
        assert_eq!(out, "\"978-0-123456-78-9\"*");
    }

    #[tokio::test]
    async fn search_books_finds_by_title_and_ranks_by_bm25() {
        let _covers = CoversTempDir::new("fts_title");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed(
                    "a.epub",
                    Some("Harry Potter"),
                    &["J.K. Rowling"],
                    &[],
                    None,
                    None,
                ),
                indexed(
                    "b.epub",
                    Some("Something Else"),
                    &["Author B"],
                    &["harry"],
                    None,
                    None,
                ),
            ],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "harry").await.unwrap();
        // Column filter scopes MATCH to title/authors/series — the tag-only
        // hit on "Something Else" is intentionally excluded.
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("Harry Potter"));
    }

    #[tokio::test]
    async fn search_books_finds_by_author_and_scopes_to_library() {
        let _covers = CoversTempDir::new("fts_author");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib-a",
            vec![indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None)],
        )
        .await
        .unwrap();
        replace_books(
            &pool,
            "/lib-b",
            vec![indexed("b.epub", Some("B"), &["Tolkien"], &[], None, None)],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib-a", "tolkien").await.unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].title.as_deref(), Some("A"));
    }

    #[tokio::test]
    async fn search_books_empty_query_returns_empty_vec() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let hits = search_books(&pool, "/lib", "   ").await.unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_books_handles_unbalanced_quote_without_error() {
        let _covers = CoversTempDir::new("fts_unbalanced");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Quoted"),
                &["Author"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        // Unbalanced `"` in raw input must not surface as a MATCH parse error.
        let hits = search_books(&pool, "/lib", "say \"hi")
            .await
            .expect("sanitizer guards against MATCH parse errors");
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn search_books_excludes_isbn_column_from_match() {
        // ISBN is indexed in books_fts but the search column filter scopes
        // MATCH to title/authors/series, so ISBN lookups are intentionally
        // not surfaced. When/if we re-enable ISBN search, flip this to
        // assert a hit — no migration required.
        let _covers = CoversTempDir::new("fts_isbn");
        let pool = init_db("sqlite::memory:").await.unwrap();
        let mut meta = indexed("a.epub", Some("ISBN Book"), &["A"], &[], None, None).metadata;
        meta.identifiers.push(Identifier {
            value: "978-0-123456-78-9".into(),
            scheme: Some("isbn".into()),
        });
        replace_books(
            &pool,
            "/lib",
            vec![IndexedBook {
                metadata: meta,
                cover: None,
            }],
        )
        .await
        .unwrap();

        let hits = search_books(&pool, "/lib", "978-0-123456-78-9")
            .await
            .unwrap();
        assert!(hits.is_empty());
    }

    #[tokio::test]
    async fn rename_author_updates_fts_via_trigger() {
        let _covers = CoversTempDir::new("fts_rename");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![indexed(
                "a.epub",
                Some("Book"),
                &["OldName"],
                &[],
                None,
                None,
            )],
        )
        .await
        .unwrap();

        assert_eq!(
            search_books(&pool, "/lib", "OldName").await.unwrap().len(),
            1
        );

        sqlx::query("UPDATE authors SET name = 'NewName' WHERE name = 'OldName'")
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            search_books(&pool, "/lib", "NewName").await.unwrap().len(),
            1
        );
        assert_eq!(
            search_books(&pool, "/lib", "OldName").await.unwrap().len(),
            0
        );
    }

    #[tokio::test]
    async fn reindex_keeps_fts_row_count_in_sync() {
        let _covers = CoversTempDir::new("fts_reindex");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["X"], &[], None, None),
                indexed("b.epub", Some("B"), &["Y"], &[], None, None),
            ],
        )
        .await
        .unwrap();
        // Reindex with one fewer book.
        replace_books(
            &pool,
            "/lib",
            vec![indexed("a.epub", Some("A"), &["X"], &[], None, None)],
        )
        .await
        .unwrap();

        let book_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books")
            .fetch_one(&pool)
            .await
            .unwrap();
        let fts_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM books_fts")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(book_count, 1);
        assert_eq!(fts_count, 1, "FTS row count must match book count");
    }

    #[tokio::test]
    async fn list_books_filters_by_author_join() {
        let _covers = CoversTempDir::new("filter_author");
        let pool = init_db("sqlite::memory:").await.unwrap();
        replace_books(
            &pool,
            "/lib",
            vec![
                indexed("a.epub", Some("A"), &["Tolkien"], &[], None, None),
                indexed("b.epub", Some("B"), &["Pratchett"], &[], None, None),
            ],
        )
        .await
        .unwrap();
        let titles: Vec<String> = sqlx::query_scalar(
            "SELECT b.title FROM books b
             JOIN books_authors_link bal ON bal.book = b.id
             JOIN authors a ON a.id = bal.author
             WHERE a.name = ?",
        )
        .bind("Tolkien")
        .fetch_all(&pool)
        .await
        .unwrap();
        assert_eq!(titles, vec!["A".to_string()]);
    }
}
