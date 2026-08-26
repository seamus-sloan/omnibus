//! The indexer's backfill passes: synthetic audiobook chapters, EPUB word
//! counts, CBZ page counts, and cover thumbnails — each filling only its
//! null/stale rows and idempotent once caught up.

use crate::pool::init_db;
use crate::test_support::{build_m4b_with_chapters, build_stored_zip, make_test_dir, EnvVarGuard};

use super::super::*;

/// Seed one audiobook `book_files` row with a part but no chapters.
///
/// `file_root` is the file's own `book_files.library_path`. `None` is the
/// unmerged shape (the file lives under its book's scan root); `Some(root)`
/// is what `merge_books` leaves behind — the row re-parented onto a book in
/// `library_path` while the bytes stay under `root`.
async fn seed_audiobook_for_backfill(
    pool: &SqlitePool,
    library_path: &str,
    file_root: Option<&str>,
    uuid: &str,
    first_part_filename: &str,
    format: &str,
) -> i64 {
    sqlx::query(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) \
         ON CONFLICT(path) DO NOTHING",
    )
    .bind(library_path)
    .bind(library_path)
    .execute(pool)
    .await
    .unwrap();

    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(library_path)
        .fetch_one(pool)
        .await
        .unwrap();

    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title, sort) \
         VALUES (?, ?, ?, ?, ?) RETURNING id",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(library_path)
    .bind(uuid)
    .bind(uuid)
    .fetch_one(pool)
    .await
    .unwrap();

    let book_file_id: i64 = sqlx::query_scalar(
        "INSERT INTO book_files (book_id, format, filename, size_bytes, mtime_epoch, library_path) \
         VALUES (?, ?, ?, 100, 100, ?) RETURNING id",
    )
    .bind(book_id)
    .bind(format)
    .bind(uuid)
    .bind(file_root)
    .fetch_one(pool)
    .await
    .unwrap();

    sqlx::query(
        "INSERT INTO book_file_parts \
            (book_file_id, ordinal, filename, size_bytes, mtime_epoch, duration_seconds) \
         VALUES (?, 0, ?, 100, 100, 3600.0)",
    )
    .bind(book_file_id)
    .bind(first_part_filename)
    .execute(pool)
    .await
    .unwrap();

    book_file_id
}

#[tokio::test]
async fn backfill_chapters_inserts_synthetic_chapters_for_all_books_in_batch() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = "/tmp/backfill_test_lib";

    let bfid_a =
        seed_audiobook_for_backfill(&pool, lib, None, "book-a", "book-a/part.m4b", "M4B").await;
    let bfid_b =
        seed_audiobook_for_backfill(&pool, lib, None, "book-b", "book-b/part.m4b", "M4B").await;

    let mut progress_calls: Vec<(u32, u32)> = Vec::new();
    let mut items: Vec<String> = Vec::new();
    backfill_chapters(&pool, lib, |processed, total, item| {
        progress_calls.push((processed, total));
        items.push(item.to_string());
    })
    .await
    .unwrap();

    let chapters_a: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM file_chapters WHERE book_file_id = ?")
            .bind(bfid_a)
            .fetch_one(&pool)
            .await
            .unwrap();
    let chapters_b: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM file_chapters WHERE book_file_id = ?")
            .bind(bfid_b)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert!(
        chapters_a >= 1,
        "book-a must have at least one synthetic chapter after backfill"
    );
    assert!(
        chapters_b >= 1,
        "book-b must have at least one synthetic chapter after backfill"
    );

    assert_eq!(
        progress_calls.len(),
        2,
        "on_progress must be called once per book"
    );
    assert_eq!(progress_calls[0], (1, 2));
    assert_eq!(progress_calls[1], (2, 2));
    // Item paths are the library directory name plus the part's relative
    // path — never the absolute `/tmp/...` root.
    assert_eq!(
        items,
        vec![
            "backfill_test_lib/book-a/part.m4b".to_string(),
            "backfill_test_lib/book-b/part.m4b".to_string(),
        ]
    );
}

#[tokio::test]
async fn backfill_chapters_is_idempotent_after_all_books_have_chapters() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let lib = "/tmp/backfill_idempotent_lib";

    let bfid =
        seed_audiobook_for_backfill(&pool, lib, None, "book-c", "book-c/part.m4b", "M4B").await;

    sqlx::query(
        "INSERT INTO file_chapters \
            (book_file_id, ordinal, title, start_seconds, duration_seconds) \
         VALUES (?, 0, 'Chapter 1', 0.0, 3600.0)",
    )
    .bind(bfid)
    .execute(&pool)
    .await
    .unwrap();

    let mut progress_calls = 0u32;
    backfill_chapters(&pool, lib, |_, _, _| {
        progress_calls += 1;
    })
    .await
    .unwrap();

    assert_eq!(
        progress_calls, 0,
        "on_progress must not be called when all books already have chapters"
    );
}

/// A merged audiobook — one whose `book_files` row was re-parented onto a
/// book under a *different* scan root by `merge_books`, keeping its own
/// `library_path`. Both halves of the resolution are under test at once: the
/// candidate query must select the row by the file's root rather than the
/// book's, and extraction must read the bytes at that same root. Failing the
/// first yields no chapters at all; failing the second yields the synthetic
/// `Part N` fallback over the book's real ones.
#[tokio::test]
async fn backfill_chapters_extracts_real_chapters_for_an_audiobook_merged_into_another_root() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let audio_root = make_test_dir("backfill-merged-audio");
    let ebook_root = make_test_dir("backfill-merged-ebook");
    let audio_lib = audio_root.to_str().unwrap();
    let ebook_lib = ebook_root.to_str().unwrap();

    // The real file lands under the AUDIO root; nothing is written under the
    // ebook root, so a stat there can only come back empty.
    let part = "merged/part.m4b";
    let abs = audio_root.join(part);
    std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
    std::fs::write(
        &abs,
        build_m4b_with_chapters(&[(0, "Opening"), (30_000_000, "Descent")]),
    )
    .unwrap();

    let book_file_id = seed_audiobook_for_backfill(
        &pool,
        ebook_lib,
        Some(audio_lib),
        "merged-book",
        part,
        "M4B",
    )
    .await;

    backfill_chapters(&pool, audio_lib, |_, _, _| {})
        .await
        .unwrap();

    let titles: Vec<String> = sqlx::query_scalar(
        "SELECT title FROM file_chapters WHERE book_file_id = ? ORDER BY ordinal",
    )
    .bind(book_file_id)
    .fetch_all(&pool)
    .await
    .unwrap();

    assert_eq!(
        titles,
        vec!["Opening".to_string(), "Descent".to_string()],
        "a merged audiobook must be selected by the backfill and read at its own library_path"
    );
}

// ---------- word-count backfill (migration 0049) ----------

/// Seed an EPUB book backed by a real fixture file on disk, with a NULL
/// `word_count` (the pre-0049 state), so `backfill_word_counts` has a live
/// file to open and a candidate row to fill. `dir` is the `scan_roots.path`
/// the backfill is scoped to; the file lands at `dir/<fixture>`.
async fn seed_ebook_missing_word_count(
    pool: &SqlitePool,
    dir: &std::path::Path,
    fixture_name: &str,
    uuid: &str,
) -> i64 {
    crate::ebook::test_support::copy_fixture_into(fixture_name, dir);
    sqlx::query(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) ON CONFLICT(path) DO NOTHING",
    )
    .bind(dir.to_str().unwrap())
    .bind(dir.to_str().unwrap())
    .execute(pool)
    .await
    .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(dir.to_str().unwrap())
        .fetch_one(pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, library_id, path, title) VALUES (?, ?, '', ?) RETURNING id",
    )
    .bind(uuid)
    .bind(lib_id)
    .bind(uuid)
    .fetch_one(pool)
    .await
    .unwrap();
    let stem = fixture_name.trim_end_matches(".epub");
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) VALUES (?, 'EPUB', ?, 0)",
    )
    .bind(book_id)
    .bind(stem)
    .execute(pool)
    .await
    .unwrap();
    book_id
}

async fn word_count_of(pool: &SqlitePool, book_id: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT word_count FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn backfill_word_counts_fills_null_rows_from_the_epub_spine() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("wc-backfill");
    // alpha.epub is a 4-word spine, beta.epub a 10-word spine (see
    // `ebook::wordcount::tests`).
    let a = seed_ebook_missing_word_count(&pool, &dir, "alpha.epub", "uuid-a").await;
    let b = seed_ebook_missing_word_count(&pool, &dir, "beta.epub", "uuid-b").await;

    let mut progress: Vec<(u32, u32)> = Vec::new();
    backfill_word_counts(&pool, dir.to_str().unwrap(), |p, t, _| {
        progress.push((p, t))
    })
    .await
    .unwrap();

    assert_eq!(word_count_of(&pool, a).await, Some(4));
    assert_eq!(word_count_of(&pool, b).await, Some(10));
    assert_eq!(progress, vec![(1, 2), (2, 2)]);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn backfill_word_counts_is_idempotent_once_filled() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("wc-backfill-idempotent");
    seed_ebook_missing_word_count(&pool, &dir, "alpha.epub", "uuid-a").await;

    backfill_word_counts(&pool, dir.to_str().unwrap(), |_, _, _| {})
        .await
        .unwrap();

    // Second pass: every book now has a count, so there are no candidates
    // and `on_progress` never fires.
    let mut calls = 0u32;
    backfill_word_counts(&pool, dir.to_str().unwrap(), |_, _, _| calls += 1)
        .await
        .unwrap();
    assert_eq!(calls, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- page-count backfill (migration 0063) ----------

/// Seed a CBZ book backed by a real (stored, uncompressed) archive on disk,
/// with a NULL `page_count` (the pre-0063 state), so `backfill_page_counts`
/// has a live file to open and a candidate row to fill.
async fn seed_cbz_missing_page_count(
    pool: &SqlitePool,
    dir: &std::path::Path,
    uuid: &str,
    pages: usize,
) -> i64 {
    let lib_str = dir.to_str().unwrap();
    sqlx::query(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) ON CONFLICT(path) DO NOTHING",
    )
    .bind(lib_str)
    .bind(lib_str)
    .execute(pool)
    .await
    .unwrap();
    let lib_id: i64 = sqlx::query_scalar("SELECT id FROM scan_roots WHERE path = ?")
        .bind(lib_str)
        .fetch_one(pool)
        .await
        .unwrap();
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title) \
         VALUES (?, ?, ?, '', ?) RETURNING id",
    )
    .bind(uuid)
    .bind(format!("{uuid}.cbz"))
    .bind(lib_id)
    .bind(uuid)
    .fetch_one(pool)
    .await
    .unwrap();
    let entries: Vec<(String, Vec<u8>)> = (0..pages)
        .map(|i| (format!("p{i}.jpg"), b"x".to_vec()))
        .collect();
    let entry_refs: Vec<(&str, &[u8])> = entries
        .iter()
        .map(|(n, d)| (n.as_str(), d.as_slice()))
        .collect();
    let bytes = build_stored_zip(&entry_refs);
    sqlx::query(
        "INSERT INTO book_files (book_id, format, filename, size_bytes) VALUES (?, 'CBZ', ?, ?)",
    )
    .bind(book_id)
    .bind(uuid)
    .bind(bytes.len() as i64)
    .execute(pool)
    .await
    .unwrap();
    std::fs::write(dir.join(format!("{uuid}.cbz")), &bytes).unwrap();
    book_id
}

async fn page_count_of(pool: &SqlitePool, book_id: i64) -> Option<i64> {
    sqlx::query_scalar("SELECT page_count FROM books WHERE id = ?")
        .bind(book_id)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn backfill_page_counts_fills_null_rows_from_the_cbz_archive() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("pc-backfill");
    let a = seed_cbz_missing_page_count(&pool, &dir, "uuid-a", 3).await;
    let b = seed_cbz_missing_page_count(&pool, &dir, "uuid-b", 7).await;

    let mut progress: Vec<(u32, u32)> = Vec::new();
    backfill_page_counts(&pool, dir.to_str().unwrap(), |p, t, _| {
        progress.push((p, t))
    })
    .await
    .unwrap();

    assert_eq!(page_count_of(&pool, a).await, Some(3));
    assert_eq!(page_count_of(&pool, b).await, Some(7));
    assert_eq!(progress, vec![(1, 2), (2, 2)]);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn backfill_page_counts_is_idempotent_once_filled() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("pc-backfill-idempotent");
    seed_cbz_missing_page_count(&pool, &dir, "uuid-a", 3).await;

    backfill_page_counts(&pool, dir.to_str().unwrap(), |_, _, _| {})
        .await
        .unwrap();

    // Second pass: every book now has a count, so there are no candidates
    // and `on_progress` never fires.
    let mut calls = 0u32;
    backfill_page_counts(&pool, dir.to_str().unwrap(), |_, _, _| calls += 1)
        .await
        .unwrap();
    assert_eq!(calls, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

#[tokio::test]
async fn backfill_page_counts_is_a_noop_when_no_candidates_exist() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("pc-backfill-empty");
    sqlx::query("INSERT INTO scan_roots (path, display_name) VALUES (?, ?)")
        .bind(dir.to_str().unwrap())
        .bind(dir.to_str().unwrap())
        .execute(&pool)
        .await
        .unwrap();

    let mut calls = 0u32;
    backfill_page_counts(&pool, dir.to_str().unwrap(), |_, _, _| calls += 1)
        .await
        .unwrap();
    assert_eq!(calls, 0);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------- thumbnail backfill (#1752 / #1817 phantom-progress fix) ----------

/// Seed a `has_cover = 1` book with a real cover file on disk, so
/// `backfill_thumbs` has bytes to (maybe) re-encode. `last_modified` is
/// pinned far in the past so any thumbnail written just now is unambiguously
/// fresher than it, regardless of clock resolution.
async fn seed_covered_book(pool: &SqlitePool, lib_id: i64, uuid: &str, title: &str) -> i64 {
    let book_id: i64 = sqlx::query_scalar(
        "INSERT INTO books (uuid, scan_key, library_id, path, title, sort, has_cover, last_modified) \
         VALUES (?, ?, ?, '', ?, ?, 1, 1) RETURNING id",
    )
    .bind(uuid)
    .bind(uuid)
    .bind(lib_id)
    .bind(title)
    .bind(title)
    .fetch_one(pool)
    .await
    .unwrap();
    std::fs::write(
        crate::covers_dir().join(format!("{uuid}.png")),
        crate::ebook::test_support::solid_color_png(200, 40, 40, 8, 8),
    )
    .unwrap();
    book_id
}

/// Write sentinel bytes (never produced by a real encode) at all three
/// thumbnail sizes for `book_id`, so it reads as fresh relative to its
/// far-past `last_modified` and a would-be re-encode is detectable.
fn write_fresh_sentinel_thumbs(book_id: i64) {
    let sentinel = b"not-a-real-webp-sentinel".to_vec();
    for size in crate::thumbs::ThumbSize::all() {
        std::fs::write(crate::thumbs::thumb_path_for(book_id, size), &sentinel).unwrap();
    }
}

/// AC1: a library whose covers are all already fresh posts no visible
/// progress — `on_progress` never fires and `backfill_thumbs` returns
/// without ever calling into the encode path.
#[tokio::test]
async fn backfill_thumbs_reports_no_progress_when_every_cover_is_already_fresh() {
    let covers_dir = make_test_dir("thumb-backfill-all-fresh-covers");
    let thumbs_dir = make_test_dir("thumb-backfill-all-fresh-thumbs");
    let _env = EnvVarGuard::set_os("OMNIBUS_COVERS_DIR", Some(covers_dir.as_os_str()))
        .also_set_os("OMNIBUS_THUMBS_DIR", Some(thumbs_dir.as_os_str()));
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("thumb-backfill-all-fresh-lib");
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) RETURNING id",
    )
    .bind(dir.to_str().unwrap())
    .bind(dir.to_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();

    let a = seed_covered_book(&pool, lib_id, "uuid-fresh-a", "Fresh A").await;
    let b = seed_covered_book(&pool, lib_id, "uuid-fresh-b", "Fresh B").await;
    write_fresh_sentinel_thumbs(a);
    write_fresh_sentinel_thumbs(b);

    let mut calls = 0u32;
    backfill_thumbs(&pool, dir.to_str().unwrap(), |_, _, _| calls += 1)
        .await
        .unwrap();

    assert_eq!(
        calls, 0,
        "on_progress must not fire when every candidate is already fresh"
    );

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&covers_dir);
    let _ = std::fs::remove_dir_all(&thumbs_dir);
}

/// AC2: a library where every cover is stale reports `count = N` and calls
/// `on_progress` exactly N times with `total = N`.
#[tokio::test]
async fn backfill_thumbs_reports_count_matching_the_stale_set_when_all_covers_are_stale() {
    let covers_dir = make_test_dir("thumb-backfill-all-stale-covers");
    let thumbs_dir = make_test_dir("thumb-backfill-all-stale-thumbs");
    let _env = EnvVarGuard::set_os("OMNIBUS_COVERS_DIR", Some(covers_dir.as_os_str()))
        .also_set_os("OMNIBUS_THUMBS_DIR", Some(thumbs_dir.as_os_str()));
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("thumb-backfill-all-stale-lib");
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) RETURNING id",
    )
    .bind(dir.to_str().unwrap())
    .bind(dir.to_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();

    // No thumbnails written on disk at all, so every size for every book is
    // stale (a missing thumbnail counts as stale).
    seed_covered_book(&pool, lib_id, "uuid-stale-a", "Stale A").await;
    seed_covered_book(&pool, lib_id, "uuid-stale-b", "Stale B").await;

    let mut progress: Vec<(u32, u32)> = Vec::new();
    backfill_thumbs(&pool, dir.to_str().unwrap(), |p, t, _| {
        progress.push((p, t))
    })
    .await
    .unwrap();

    assert_eq!(progress, vec![(1, 2), (2, 2)]);

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&covers_dir);
    let _ = std::fs::remove_dir_all(&thumbs_dir);
}

/// AC4: a library with both fresh and stale covers reports progress only
/// for the stale ones — the mixed case the phantom-progress bug collapsed
/// into "report every book with a cover".
#[tokio::test]
async fn backfill_thumbs_reports_progress_only_for_stale_covers_in_a_mixed_library() {
    let covers_dir = make_test_dir("thumb-backfill-mixed-covers");
    let thumbs_dir = make_test_dir("thumb-backfill-mixed-thumbs");
    let _env = EnvVarGuard::set_os("OMNIBUS_COVERS_DIR", Some(covers_dir.as_os_str()))
        .also_set_os("OMNIBUS_THUMBS_DIR", Some(thumbs_dir.as_os_str()));
    let pool = init_db("sqlite::memory:").await.unwrap();
    let dir = make_test_dir("thumb-backfill-mixed-lib");
    let lib_id: i64 = sqlx::query_scalar(
        "INSERT INTO scan_roots (path, display_name) VALUES (?, ?) RETURNING id",
    )
    .bind(dir.to_str().unwrap())
    .bind(dir.to_str().unwrap())
    .fetch_one(&pool)
    .await
    .unwrap();

    let fresh = seed_covered_book(&pool, lib_id, "uuid-mixed-fresh", "Mixed Fresh").await;
    write_fresh_sentinel_thumbs(fresh);
    // No thumbnails written for this one, so it's the sole stale candidate.
    seed_covered_book(&pool, lib_id, "uuid-mixed-stale", "Mixed Stale").await;

    let mut titles: Vec<String> = Vec::new();
    let mut progress: Vec<(u32, u32)> = Vec::new();
    backfill_thumbs(&pool, dir.to_str().unwrap(), |p, t, title| {
        progress.push((p, t));
        titles.push(title.to_string());
    })
    .await
    .unwrap();

    assert_eq!(
        progress,
        vec![(1, 1)],
        "total and processed must both reflect the single stale book, not the two candidates"
    );
    assert_eq!(
        titles,
        vec!["Mixed Stale".to_string()],
        "on_progress must not fire for the already-fresh book"
    );

    let sentinel = b"not-a-real-webp-sentinel".to_vec();
    for size in crate::thumbs::ThumbSize::all() {
        let on_disk = std::fs::read(crate::thumbs::thumb_path_for(fresh, size)).unwrap();
        assert_eq!(
            on_disk, sentinel,
            "the already-fresh book's thumbnail for size {size} must not be re-encoded"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
    let _ = std::fs::remove_dir_all(&covers_dir);
    let _ = std::fs::remove_dir_all(&thumbs_dir);
}
