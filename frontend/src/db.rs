use sqlx::{sqlite::SqlitePoolOptions, Row, SqlitePool};

pub use omnibus_shared::Settings;
use omnibus_shared::{Contributor, EbookLibrary, EbookMetadata, Identifier};

pub async fn init_db(database_url: &str) -> Result<SqlitePool, sqlx::Error> {
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(database_url)
        .await?;

    initialize_schema(&pool).await?;
    Ok(pool)
}

pub async fn initialize_schema(pool: &SqlitePool) -> Result<(), sqlx::Error> {
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS app_state (
            id INTEGER PRIMARY KEY CHECK(id = 1),
            value INTEGER NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        INSERT INTO app_state (id, value)
        SELECT 1, 0
        WHERE NOT EXISTS (SELECT 1 FROM app_state WHERE id = 1)
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS settings (
            key   TEXT PRIMARY KEY,
            value TEXT NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    // Cached book metadata. The landing page queries this table instead of
    // walking the filesystem on every request. Rows are keyed by the library
    // root they were indexed under + the file's relative path so indexing a
    // new library (via settings change) doesn't conflict with an old one.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS books (
            id                INTEGER PRIMARY KEY AUTOINCREMENT,
            library_path      TEXT NOT NULL,
            filename          TEXT NOT NULL,
            title             TEXT,
            description       TEXT,
            publisher         TEXT,
            published         TEXT,
            modified          TEXT,
            language          TEXT,
            rights            TEXT,
            source            TEXT,
            coverage          TEXT,
            dc_type           TEXT,
            dc_format         TEXT,
            relation          TEXT,
            creators_json     TEXT NOT NULL DEFAULT '[]',
            contributors_json TEXT NOT NULL DEFAULT '[]',
            subjects_json     TEXT NOT NULL DEFAULT '[]',
            identifiers_json  TEXT NOT NULL DEFAULT '[]',
            series            TEXT,
            series_index      TEXT,
            epub_version      TEXT,
            unique_identifier TEXT,
            resource_count    INTEGER NOT NULL DEFAULT 0,
            spine_count       INTEGER NOT NULL DEFAULT 0,
            toc_count         INTEGER NOT NULL DEFAULT 0,
            has_cover         INTEGER NOT NULL DEFAULT 0,
            error             TEXT,
            UNIQUE(library_path, filename)
        )
        "#,
    )
    .execute(pool)
    .await?;

    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS book_covers (
            book_id INTEGER PRIMARY KEY REFERENCES books(id) ON DELETE CASCADE,
            mime    TEXT NOT NULL,
            bytes   BLOB NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    // Tracks the last successful index per library path so callers can
    // decide when to kick off a refresh.
    sqlx::query(
        r#"
        CREATE TABLE IF NOT EXISTS library_index_state (
            library_path TEXT PRIMARY KEY,
            last_indexed INTEGER NOT NULL
        )
        "#,
    )
    .execute(pool)
    .await?;

    Ok(())
}

fn decode_json<T: serde::de::DeserializeOwned + Default>(s: &str) -> T {
    serde_json::from_str(s).unwrap_or_default()
}

/// Return every book that was indexed under `library_path`, ordered by
/// filename. Rows without a cover get `cover_url = None`; rows with one get
/// a relative `/api/covers/:id` URL that clients combine with their server
/// base.
pub async fn list_books(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Vec<EbookMetadata>, sqlx::Error> {
    let rows = sqlx::query(
        r#"
        SELECT id, filename, title, description, publisher, published, modified,
               language, rights, source, coverage, dc_type, dc_format, relation,
               creators_json, contributors_json, subjects_json, identifiers_json,
               series, series_index, epub_version, unique_identifier,
               resource_count, spine_count, toc_count, has_cover, error
        FROM books
        WHERE library_path = ?
        ORDER BY filename
        "#,
    )
    .bind(library_path)
    .fetch_all(pool)
    .await?;

    let books = rows
        .into_iter()
        .map(|r| {
            let id: i64 = r.get("id");
            let has_cover: i64 = r.get("has_cover");
            let creators: Vec<Contributor> = decode_json(&r.get::<String, _>("creators_json"));
            let contributors: Vec<Contributor> =
                decode_json(&r.get::<String, _>("contributors_json"));
            let subjects: Vec<String> = decode_json(&r.get::<String, _>("subjects_json"));
            let identifiers: Vec<Identifier> = decode_json(&r.get::<String, _>("identifiers_json"));
            let resource_count: i64 = r.get("resource_count");
            let spine_count: i64 = r.get("spine_count");
            let toc_count: i64 = r.get("toc_count");
            EbookMetadata {
                id,
                filename: r.get("filename"),
                title: r.get("title"),
                description: r.get("description"),
                publisher: r.get("publisher"),
                published: r.get("published"),
                modified: r.get("modified"),
                language: r.get("language"),
                rights: r.get("rights"),
                source: r.get("source"),
                coverage: r.get("coverage"),
                dc_type: r.get("dc_type"),
                dc_format: r.get("dc_format"),
                relation: r.get("relation"),
                creators,
                contributors,
                subjects,
                identifiers,
                series: r.get("series"),
                series_index: r.get("series_index"),
                epub_version: r.get("epub_version"),
                unique_identifier: r.get("unique_identifier"),
                resource_count: resource_count as usize,
                spine_count: spine_count as usize,
                toc_count: toc_count as usize,
                cover_url: (has_cover != 0).then(|| format!("/api/covers/{id}")),
                error: r.get("error"),
            }
        })
        .collect();

    Ok(books)
}

/// Load a book's cover image bytes + mime type. Used by the `/api/covers/:id`
/// route.
pub async fn get_cover(
    pool: &SqlitePool,
    book_id: i64,
) -> Result<Option<(String, Vec<u8>)>, sqlx::Error> {
    let row = sqlx::query("SELECT mime, bytes FROM book_covers WHERE book_id = ?")
        .bind(book_id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| (r.get::<String, _>("mime"), r.get::<Vec<u8>, _>("bytes"))))
}

/// Atomically replace every book row for `library_path` with `books` and
/// record the index time. Callers should have already serialized the scan;
/// this just writes the result.
pub async fn replace_books(
    pool: &SqlitePool,
    library_path: &str,
    books: Vec<crate::ebook::IndexedBook>,
) -> Result<(), sqlx::Error> {
    let mut tx = pool.begin().await?;

    sqlx::query("DELETE FROM books WHERE library_path = ?")
        .bind(library_path)
        .execute(&mut *tx)
        .await?;

    for b in books {
        let m = &b.metadata;
        let creators = serde_json::to_string(&m.creators).unwrap_or_else(|_| "[]".into());
        let contributors = serde_json::to_string(&m.contributors).unwrap_or_else(|_| "[]".into());
        let subjects = serde_json::to_string(&m.subjects).unwrap_or_else(|_| "[]".into());
        let identifiers = serde_json::to_string(&m.identifiers).unwrap_or_else(|_| "[]".into());
        let has_cover = if b.cover.is_some() { 1 } else { 0 };

        let id = sqlx::query(
            r#"
            INSERT INTO books (
                library_path, filename, title, description, publisher, published,
                modified, language, rights, source, coverage, dc_type, dc_format,
                relation, creators_json, contributors_json, subjects_json,
                identifiers_json, series, series_index, epub_version,
                unique_identifier, resource_count, spine_count, toc_count,
                has_cover, error
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                      ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(library_path)
        .bind(&m.filename)
        .bind(&m.title)
        .bind(&m.description)
        .bind(&m.publisher)
        .bind(&m.published)
        .bind(&m.modified)
        .bind(&m.language)
        .bind(&m.rights)
        .bind(&m.source)
        .bind(&m.coverage)
        .bind(&m.dc_type)
        .bind(&m.dc_format)
        .bind(&m.relation)
        .bind(creators)
        .bind(contributors)
        .bind(subjects)
        .bind(identifiers)
        .bind(&m.series)
        .bind(&m.series_index)
        .bind(&m.epub_version)
        .bind(&m.unique_identifier)
        .bind(m.resource_count as i64)
        .bind(m.spine_count as i64)
        .bind(m.toc_count as i64)
        .bind(has_cover)
        .bind(&m.error)
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();

        if let Some((mime, bytes)) = b.cover {
            sqlx::query("INSERT INTO book_covers (book_id, mime, bytes) VALUES (?, ?, ?)")
                .bind(id)
                .bind(mime)
                .bind(bytes)
                .execute(&mut *tx)
                .await?;
        }
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    sqlx::query(
        r#"
        INSERT INTO library_index_state (library_path, last_indexed)
        VALUES (?, ?)
        ON CONFLICT(library_path) DO UPDATE SET last_indexed = excluded.last_indexed
        "#,
    )
    .bind(library_path)
    .bind(now)
    .execute(&mut *tx)
    .await?;

    tx.commit().await
}

/// Unix-seconds timestamp of the last successful index for `library_path`,
/// or `None` if never indexed.
pub async fn last_indexed_at(
    pool: &SqlitePool,
    library_path: &str,
) -> Result<Option<i64>, sqlx::Error> {
    let ts = sqlx::query_scalar::<_, i64>(
        "SELECT last_indexed FROM library_index_state WHERE library_path = ?",
    )
    .bind(library_path)
    .fetch_optional(pool)
    .await?;
    Ok(ts)
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
    match &settings.ebook_library_path {
        Some(path) => {
            sqlx::query(
                "INSERT OR REPLACE INTO settings (key, value) VALUES ('ebook_library_path', ?)",
            )
            .bind(path)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM settings WHERE key = 'ebook_library_path'")
                .execute(pool)
                .await?;
        }
    }

    match &settings.audiobook_library_path {
        Some(path) => {
            sqlx::query(
                "INSERT OR REPLACE INTO settings (key, value) VALUES ('audiobook_library_path', ?)",
            )
            .bind(path)
            .execute(pool)
            .await?;
        }
        None => {
            sqlx::query("DELETE FROM settings WHERE key = 'audiobook_library_path'")
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn initializes_and_seeds_default_value() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let value = get_value(&pool).await.expect("seeded value should exist");
        assert_eq!(value, 0);
    }

    #[tokio::test]
    async fn increments_value_persistently() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");

        let value = increment_value(&pool)
            .await
            .expect("increment should succeed");
        assert_eq!(value, 1);

        let value = get_value(&pool).await.expect("value should be persisted");
        assert_eq!(value, 1);
    }

    #[tokio::test]
    async fn get_settings_returns_none_for_empty_db() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let settings = get_settings(&pool).await.expect("should succeed");
        assert_eq!(settings.ebook_library_path, None);
        assert_eq!(settings.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn set_and_get_settings_roundtrips() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        let input = Settings {
            ebook_library_path: Some("/books/ebooks".to_string()),
            audiobook_library_path: Some("/books/audio".to_string()),
        };
        set_settings(&pool, &input)
            .await
            .expect("set should succeed");
        let result = get_settings(&pool).await.expect("get should succeed");
        assert_eq!(result, input);
    }

    #[tokio::test]
    async fn set_settings_updates_existing_values() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/old".to_string()),
                audiobook_library_path: None,
            },
        )
        .await
        .expect("first set should succeed");
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/new".to_string()),
                audiobook_library_path: Some("/audio".to_string()),
            },
        )
        .await
        .expect("second set should succeed");
        let result = get_settings(&pool).await.expect("get should succeed");
        assert_eq!(result.ebook_library_path, Some("/new".to_string()));
        assert_eq!(result.audiobook_library_path, Some("/audio".to_string()));
    }

    #[tokio::test]
    async fn set_settings_none_clears_existing_value() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: Some("/books".to_string()),
                audiobook_library_path: Some("/audio".to_string()),
            },
        )
        .await
        .expect("set should succeed");
        set_settings(
            &pool,
            &Settings {
                ebook_library_path: None,
                audiobook_library_path: None,
            },
        )
        .await
        .expect("clear should succeed");
        let result = get_settings(&pool).await.expect("get should succeed");
        assert_eq!(result.ebook_library_path, None);
        assert_eq!(result.audiobook_library_path, None);
    }

    #[tokio::test]
    async fn seed_settings_from_env_writes_env_vars_to_db() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        // Use unique env var names per test to avoid cross-test pollution
        std::env::set_var("EBOOK_LIBRARY_PATH", "/env/books");
        std::env::set_var("AUDIOBOOK_LIBRARY_PATH", "/env/audio");
        seed_settings_from_env(&pool)
            .await
            .expect("seed should succeed");
        std::env::remove_var("EBOOK_LIBRARY_PATH");
        std::env::remove_var("AUDIOBOOK_LIBRARY_PATH");
        let result = get_settings(&pool).await.expect("get should succeed");
        assert_eq!(result.ebook_library_path, Some("/env/books".to_string()));
        assert_eq!(
            result.audiobook_library_path,
            Some("/env/audio".to_string())
        );
    }

    #[tokio::test]
    async fn seed_settings_from_env_is_noop_when_vars_unset() {
        let pool = init_db("sqlite::memory:")
            .await
            .expect("db should initialize");
        // Ensure the vars aren't set (they shouldn't be in a clean test env)
        std::env::remove_var("EBOOK_LIBRARY_PATH");
        std::env::remove_var("AUDIOBOOK_LIBRARY_PATH");
        seed_settings_from_env(&pool)
            .await
            .expect("seed should succeed");
        let result = get_settings(&pool).await.expect("get should succeed");
        assert_eq!(result.ebook_library_path, None);
        assert_eq!(result.audiobook_library_path, None);
    }
}
