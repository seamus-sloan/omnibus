//! Tests for cross-format links and the linear mapping tier: link CRUD and
//! snapshot staleness, sequence/narrations timelines, percent ↔ seconds
//! mapping with inverse consistency, and the resume-candidate state machine.
//! Split by sub-topic into the sibling modules below; shared fixtures live
//! here.

mod alignment;
mod anchors;
mod cfi;
mod declare;
mod links;
mod resume;
mod timeline;

use omnibus_shared::cross_format::DeclareSyncPoint;
use omnibus_shared::{ProgressFormat, ProgressUpdate};
use sqlx::SqlitePool;

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

/// One dual-format book: an EPUB file plus one M4B `book_files` row per
/// entry of `durations` (ordinals in order, one part each). Returns
/// `(book_id, uuid, audio book_file ids)`.
async fn seed_dual_book(pool: &SqlitePool, durations: &[f64]) -> (i64, String, Vec<i64>) {
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES ('/lib', 'lib')")
        .execute(pool)
        .await
        .unwrap();
    let library_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = '/lib'")
        .fetch_one(pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort)
         VALUES ('book-uuid-1', 'b1.epub', ?, '/lib/b1', 'Dual', 'dual') RETURNING id",
    )
    .bind(library_id)
    .fetch_one(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch, scan_key)
         VALUES (?, 'EPUB', 'b1', 10, 10, 'b1.epub')",
    )
    .bind(book_id)
    .execute(pool)
    .await
    .unwrap();
    let mut audio_ids = Vec::new();
    for (i, duration) in durations.iter().enumerate() {
        let file_id: i64 = sqlx::query_scalar(
            "INSERT INTO book_files
                (book_id, format, filename, size_bytes, mtime_epoch, scan_key, ordinal)
             VALUES (?, 'M4B', ?, ?, ?, ?, ?) RETURNING id",
        )
        .bind(book_id)
        .bind(format!("part{i}"))
        .bind(100 + i as i64)
        .bind(1_000 + i as i64)
        .bind(format!("a{i}.m4b"))
        .bind(i as i64)
        .fetch_one(pool)
        .await
        .unwrap();
        sqlx::query(
            "INSERT INTO book_file_parts
                (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds)
             VALUES (?, 0, ?, ?, ?, ?)",
        )
        .bind(file_id)
        .bind(format!("a{i}.m4b"))
        .bind(100 + i as i64)
        .bind(1_000 + i as i64)
        .bind(duration)
        .execute(pool)
        .await
        .unwrap();
        audio_ids.push(file_id);
    }
    (book_id, "book-uuid-1".to_string(), audio_ids)
}

fn epub_percent_update(uuid: &str, percent: i64, clock: i64) -> ProgressUpdate {
    ProgressUpdate {
        book_uuid: uuid.to_string(),
        format: ProgressFormat::Epub,
        epub_cfi: None,
        audio_position_seconds: None,
        progress_percent: Some(percent),
        kobo_location: None,
        book_file_id: None,
        client_updated_at: Some(clock),
    }
}

fn audio_update(uuid: &str, seconds: f64, file_id: Option<i64>, clock: i64) -> ProgressUpdate {
    ProgressUpdate {
        book_uuid: uuid.to_string(),
        format: ProgressFormat::Audio,
        epub_cfi: None,
        audio_position_seconds: Some(seconds),
        progress_percent: None,
        kobo_location: None,
        book_file_id: file_id,
        client_updated_at: Some(clock),
    }
}

/// A `declare_sync_point` payload naming an audio-side position — shared by
/// the reconfirm/reorder tests (`links`) and the declare-mechanics tests
/// (`declare`).
fn declare_audio(uuid: &str, file_id: Option<i64>, seconds: f64) -> DeclareSyncPoint {
    DeclareSyncPoint {
        book_uuid: uuid.to_string(),
        format: ProgressFormat::Audio,
        ebook_fraction: None,
        epub_cfi: None,
        audio_book_file_id: file_id,
        audio_seconds: Some(seconds),
    }
}

/// Replace the book's EPUB spine/TOC structure with `titles`, each chapter
/// `per_chapter` visible chars long — shared by the alignment-view and
/// chapter-anchoring tests.
async fn seed_epub_chapters(pool: &SqlitePool, titles: &[(&str, i64)], per_chapter: i64) {
    let epub_file: i64 =
        sqlx::query_scalar("SELECT id FROM book_files WHERE format = 'EPUB' LIMIT 1")
            .fetch_one(pool)
            .await
            .unwrap();
    let spine = titles
        .iter()
        .enumerate()
        .map(|(i, _)| crate::ebook::toc::SpineStat {
            spine_index: i as i64,
            href: format!("c{i}.xhtml"),
            visible_chars: per_chapter,
        })
        .collect();
    let chapters = titles
        .iter()
        .enumerate()
        .map(|(i, (title, start))| crate::ebook::toc::TocChapter {
            ordinal: i as i64,
            title: (*title).to_string(),
            href: format!("c{i}.xhtml"),
            spine_index: i as i64,
            start_chars: *start,
        })
        .collect();
    crate::epub_structure::replace_structure(
        pool,
        epub_file,
        &crate::ebook::toc::EpubStructure { spine, chapters },
    )
    .await
    .unwrap();
}

/// Seed one audio file's `file_chapters` rows from `(title, start_seconds)`
/// pairs — the counterpart to [`seed_epub_chapters`] for the audio side.
async fn seed_audio_chapters(pool: &SqlitePool, file_id: i64, chapters: &[(&str, f64)]) {
    for (i, (title, start)) in chapters.iter().enumerate() {
        sqlx::query(
            "INSERT INTO file_chapters (book_file_id, ordinal, title, start_seconds, duration_seconds)
             VALUES (?, ?, ?, ?, 0)",
        )
        .bind(file_id)
        .bind(i as i64)
        .bind(title)
        .bind(start)
        .execute(pool)
        .await
        .unwrap();
    }
}
