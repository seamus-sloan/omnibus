//! F2.1 progress sync — server-authoritative reading/listening position and
//! batched session reports.
//!
//! Position upserts are last-write-wins on `(user_id, book_id, format)`; the
//! upsert always bumps `updated_at` to now so a newly-opened client can sync
//! forward by reading the row back. Session inserts write into the
//! per-format `reading_sessions` / `listening_sessions` tables.

use omnibus_shared::{ProgressFormat, ProgressRecord, ProgressUpdate, SessionReport};
use sqlx::{Row, SqlitePool};

use crate::resolve_book_id_by_uuid;

#[derive(Debug, thiserror::Error)]
pub enum ProgressError {
    #[error("book not found")]
    BookNotFound,
    #[error(transparent)]
    Sqlx(#[from] sqlx::Error),
}

fn format_str(f: ProgressFormat) -> &'static str {
    match f {
        ProgressFormat::Epub => "epub",
        ProgressFormat::Audio => "audio",
    }
}

fn parse_format(raw: &str) -> ProgressFormat {
    // The CHECK constraint on `reading_progress.format` rules out anything
    // other than these two values, so an unknown string here would be a
    // schema-level invariant breach. Default to Epub rather than panic.
    match raw {
        "audio" => ProgressFormat::Audio,
        _ => ProgressFormat::Epub,
    }
}

/// Upsert a position row for `(user, book, format)` and return the new
/// server-authoritative record. Resolves `book_uuid` → `book_id`; returns
/// `ProgressError::BookNotFound` when the uuid is unknown.
pub async fn upsert_progress(
    pool: &SqlitePool,
    user_id: i64,
    update: &ProgressUpdate,
) -> Result<ProgressRecord, ProgressError> {
    let book_id = resolve_book_id_by_uuid(pool, &update.book_uuid)
        .await?
        .ok_or(ProgressError::BookNotFound)?;
    let fmt = format_str(update.format);
    sqlx::query(
        "INSERT INTO reading_progress
            (user_id, book_id, format, epub_cfi, audio_position_seconds, updated_at)
         VALUES (?, ?, ?, ?, ?, strftime('%s','now'))
         ON CONFLICT(user_id, book_id, format) DO UPDATE SET
             epub_cfi = excluded.epub_cfi,
             audio_position_seconds = excluded.audio_position_seconds,
             updated_at = strftime('%s','now')",
    )
    .bind(user_id)
    .bind(book_id)
    .bind(fmt)
    .bind(update.epub_cfi.as_deref())
    .bind(update.audio_position_seconds)
    .execute(pool)
    .await?;

    let row = sqlx::query(
        "SELECT epub_cfi, audio_position_seconds, updated_at
         FROM reading_progress
         WHERE user_id = ? AND book_id = ? AND format = ?",
    )
    .bind(user_id)
    .bind(book_id)
    .bind(fmt)
    .fetch_one(pool)
    .await?;
    Ok(ProgressRecord {
        book_uuid: update.book_uuid.clone(),
        format: update.format,
        epub_cfi: row.try_get::<Option<String>, _>("epub_cfi")?,
        audio_position_seconds: row.try_get::<Option<f64>, _>("audio_position_seconds")?,
        updated_at: row.try_get::<i64, _>("updated_at")?,
    })
}

/// Fetch the current position row for `(user, book_uuid, format)`. Returns
/// `Ok(None)` for an unknown book uuid or for a book with no row for that
/// format yet.
pub async fn get_progress(
    pool: &SqlitePool,
    user_id: i64,
    book_uuid: &str,
    format: ProgressFormat,
) -> Result<Option<ProgressRecord>, sqlx::Error> {
    let Some(book_id) = resolve_book_id_by_uuid(pool, book_uuid).await? else {
        return Ok(None);
    };
    let fmt = format_str(format);
    let Some(row) = sqlx::query(
        "SELECT format, epub_cfi, audio_position_seconds, updated_at
         FROM reading_progress
         WHERE user_id = ? AND book_id = ? AND format = ?",
    )
    .bind(user_id)
    .bind(book_id)
    .bind(fmt)
    .fetch_optional(pool)
    .await?
    else {
        return Ok(None);
    };
    Ok(Some(ProgressRecord {
        book_uuid: book_uuid.to_string(),
        format: parse_format(row.try_get::<String, _>("format")?.as_str()),
        epub_cfi: row.try_get::<Option<String>, _>("epub_cfi")?,
        audio_position_seconds: row.try_get::<Option<f64>, _>("audio_position_seconds")?,
        updated_at: row.try_get::<i64, _>("updated_at")?,
    }))
}

/// Append one session row to the per-format table. Silently skips reports
/// whose `book_uuid` is unknown — a session that outlived the book it
/// referred to is best-effort telemetry, not an integrity failure.
pub async fn record_session(
    pool: &SqlitePool,
    user_id: i64,
    report: &SessionReport,
) -> Result<(), sqlx::Error> {
    let Some(book_id) = resolve_book_id_by_uuid(pool, &report.book_uuid).await? else {
        return Ok(());
    };
    match report.format {
        ProgressFormat::Epub => {
            sqlx::query(
                "INSERT INTO reading_sessions
                    (user_id, book_id, started_at, ended_at, seconds_read, device_id)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(book_id)
            .bind(report.started_at)
            .bind(report.ended_at)
            .bind(report.progress_units)
            .bind(report.device_id)
            .execute(pool)
            .await?;
        }
        ProgressFormat::Audio => {
            sqlx::query(
                "INSERT INTO listening_sessions
                    (user_id, book_id, started_at, ended_at, seconds_listened, device_id)
                 VALUES (?, ?, ?, ?, ?, ?)",
            )
            .bind(user_id)
            .bind(book_id)
            .bind(report.started_at)
            .bind(report.ended_at)
            .bind(report.progress_units)
            .bind(report.device_id)
            .execute(pool)
            .await?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{init_db, replace_books};
    use omnibus_shared::EbookMetadata;

    async fn seed(pool: &SqlitePool, library: &str, title: &str) -> (i64, String) {
        replace_books(
            pool,
            library,
            vec![crate::ebook::IndexedBook {
                metadata: EbookMetadata {
                    filename: format!("{title}.epub").to_lowercase(),
                    title: Some(title.to_string()),
                    ..Default::default()
                },
                cover: None,
                mtime_epoch: 0,
                size_bytes: 0,
            }],
        )
        .await
        .expect("seed book");
        let books = crate::list_books(pool, library).await.unwrap();
        let book = books
            .into_iter()
            .find(|b| b.title.as_deref() == Some(title))
            .unwrap();
        (book.id, book.unique_identifier.clone().unwrap())
    }

    async fn seed_user(pool: &SqlitePool, name: &str) -> i64 {
        sqlx::query_scalar::<_, i64>(
            "INSERT INTO users (username, password_hash, is_admin, can_upload, can_edit, can_download)
             VALUES (?, '!x', 0, 0, 0, 1) RETURNING id",
        )
        .bind(name)
        .fetch_one(pool)
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn upsert_round_trips_epub_position() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user = seed_user(&pool, "alice").await;
        let (_, uuid) = seed(&pool, "/lib", "Book A").await;
        let upd = ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
            audio_position_seconds: None,
        };
        let saved = upsert_progress(&pool, user, &upd).await.unwrap();
        assert_eq!(saved.book_uuid, uuid);
        assert_eq!(saved.format, ProgressFormat::Epub);
        assert_eq!(saved.epub_cfi.as_deref(), Some("epubcfi(/6/4!/4/2/1:0)"));
        assert!(saved.updated_at > 0);

        let fetched = get_progress(&pool, user, &uuid, ProgressFormat::Epub)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fetched.epub_cfi, saved.epub_cfi);
    }

    #[tokio::test]
    async fn upsert_is_last_write_wins() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user = seed_user(&pool, "alice").await;
        let (_, uuid) = seed(&pool, "/lib", "Book A").await;
        let first = ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/4!/4/2/1:0)".into()),
            audio_position_seconds: None,
        };
        upsert_progress(&pool, user, &first).await.unwrap();
        let second = ProgressUpdate {
            book_uuid: uuid.clone(),
            format: ProgressFormat::Epub,
            epub_cfi: Some("epubcfi(/6/12!/4/8/3:7)".into()),
            audio_position_seconds: None,
        };
        let saved = upsert_progress(&pool, user, &second).await.unwrap();
        assert_eq!(saved.epub_cfi.as_deref(), Some("epubcfi(/6/12!/4/8/3:7)"));
    }

    #[tokio::test]
    async fn isolates_per_user_book_format() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let alice = seed_user(&pool, "alice").await;
        let bob = seed_user(&pool, "bob").await;
        let (_, uuid) = seed(&pool, "/lib", "Book A").await;
        upsert_progress(
            &pool,
            alice,
            &ProgressUpdate {
                book_uuid: uuid.clone(),
                format: ProgressFormat::Epub,
                epub_cfi: Some("epubcfi(alice)".into()),
                audio_position_seconds: None,
            },
        )
        .await
        .unwrap();
        upsert_progress(
            &pool,
            alice,
            &ProgressUpdate {
                book_uuid: uuid.clone(),
                format: ProgressFormat::Audio,
                epub_cfi: None,
                audio_position_seconds: Some(42.5),
            },
        )
        .await
        .unwrap();
        // Bob has no row yet.
        assert!(get_progress(&pool, bob, &uuid, ProgressFormat::Epub)
            .await
            .unwrap()
            .is_none());
        // Alice's two rows don't trample each other.
        let epub = get_progress(&pool, alice, &uuid, ProgressFormat::Epub)
            .await
            .unwrap()
            .unwrap();
        let audio = get_progress(&pool, alice, &uuid, ProgressFormat::Audio)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(epub.epub_cfi.as_deref(), Some("epubcfi(alice)"));
        assert_eq!(audio.audio_position_seconds, Some(42.5));
    }

    #[tokio::test]
    async fn upsert_unknown_book_is_not_found() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user = seed_user(&pool, "alice").await;
        let res = upsert_progress(
            &pool,
            user,
            &ProgressUpdate {
                book_uuid: "no-such-uuid".into(),
                format: ProgressFormat::Epub,
                epub_cfi: Some("epubcfi(x)".into()),
                audio_position_seconds: None,
            },
        )
        .await;
        assert!(matches!(res, Err(ProgressError::BookNotFound)));
    }

    #[tokio::test]
    async fn get_progress_returns_none_when_unset() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user = seed_user(&pool, "alice").await;
        let (_, uuid) = seed(&pool, "/lib", "Book A").await;
        assert!(get_progress(&pool, user, &uuid, ProgressFormat::Epub)
            .await
            .unwrap()
            .is_none());
    }

    #[tokio::test]
    async fn record_session_inserts_per_format_row() {
        let pool = init_db("sqlite::memory:").await.unwrap();
        let user = seed_user(&pool, "alice").await;
        let (book_id, uuid) = seed(&pool, "/lib", "Book A").await;
        record_session(
            &pool,
            user,
            &SessionReport {
                book_uuid: uuid.clone(),
                format: ProgressFormat::Epub,
                started_at: 100,
                ended_at: 460,
                progress_units: 360,
                device_id: None,
            },
        )
        .await
        .unwrap();
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM reading_sessions WHERE user_id = ? AND book_id = ?",
        )
        .bind(user)
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(count, 1);

        record_session(
            &pool,
            user,
            &SessionReport {
                book_uuid: uuid.clone(),
                format: ProgressFormat::Audio,
                started_at: 200,
                ended_at: 800,
                progress_units: 600,
                device_id: None,
            },
        )
        .await
        .unwrap();
        let audio_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM listening_sessions WHERE user_id = ? AND book_id = ?",
        )
        .bind(user)
        .bind(book_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(audio_count, 1);
    }
}
