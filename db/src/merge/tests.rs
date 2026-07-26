//! Unit tests for `merge_books`: file/link/log relocation to the target
//! book, taxonomy and user-data transfer, and the same-book rejection.

use omnibus_shared::MetadataOverrides;

use super::*;
use crate::books::{list_indexed_rows_for_formats, list_merged_rows_for_formats};
use crate::indexer::diff_library;
use crate::pool::init_db;
use crate::sync::{sync_audiobooks, AudiobookSyncPlan};
use crate::test_support::{
    count_rows as count, indexed_audiobook, seed_synced_audiobook as seed_audiobook,
    seed_synced_ebook as seed_ebook,
};

async fn seed_user(pool: &sqlx::SqlitePool) -> i64 {
    sqlx::query_scalar(
        "INSERT INTO users (username, password_hash, is_admin) VALUES ('admin', 'x', 1) RETURNING id",
    )
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn book_id_by_uuid(pool: &sqlx::SqlitePool, uuid: &str) -> i64 {
    sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(uuid)
        .fetch_one(pool)
        .await
        .unwrap()
}

#[tokio::test]
async fn merge_books_moves_files_links_and_records_log() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Different titles so auto-attach didn't already combine them.
    let target = seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "Stoker/Drakula audio.m4b", "Drakula", "Bram Stoker").await;
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 2);

    let out = merge_books(&pool, &source, &target, None).await.unwrap();
    assert_eq!(out.target_uuid, target);

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM book_files").await, 2);
    // Parts followed the re-parented book_files row.
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM book_file_parts").await,
        1
    );
    // The moved audio file row got the source's location stamped in so
    // HLS still resolves against the audio root.
    let (bf_lib, bf_path): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT library_path, path FROM book_files WHERE format = 'M4B'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(bf_lib.as_deref(), Some("/audio"));
    assert_eq!(bf_path.as_deref(), Some("Stoker"));
    // Reindex guard + audit row.
    let (mu_book, mu_fmt): (i64, String) =
        sqlx::query_as("SELECT book_id, format FROM merged_uuids WHERE uuid = ?")
            .bind(&source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(mu_book, book_id_by_uuid(&pool, &target).await);
    assert_eq!(mu_fmt, "M4B");
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merge_log").await, 1);
    // Author union: the audiobook's author link moved over.
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM books_authors_link").await,
        1
    );
    // FTS: the source's row is gone, the target's remains.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books_fts").await, 1);
}

/// The `move_links` series/tags/publishers/languages loop and the blind
/// `move_progress_and_history` re-parent of bookmarks/sessions/highlights are
/// only ever seeded with an author elsewhere; this covers a book carrying the
/// full taxonomy and user-data on *both* sides, so a merge moves every kind to
/// the target and drops the source's rows.
#[tokio::test]
async fn merge_books_moves_all_taxonomy_and_user_data_to_target() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    // M4B target + MP3 source: distinct file formats (pass the collision
    // check) that both map to the coarse 'audio' progress format.
    let target = seed_audiobook(&pool, "A/Dracula.m4b", "Dracula", "Bram Stoker").await;
    let mut mp3 = indexed_audiobook("B/Drakula mp3", "Drakula", Some("Bram Stoker"));
    mp3.format = "MP3".into();
    // Keep the part filename consistent with the MP3 format so the fixture
    // reads as a real MP3 source (the helper hardcodes an `.m4b` part).
    mp3.parts[0].filename = "B/Drakula mp3/part1.mp3".into();
    let source_scan_key = mp3.scan_key.clone();
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![mp3],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let source = crate::test_support::uuid_by_scan_key(&pool, &source_scan_key).await;
    let source_id = book_id_by_uuid(&pool, &source).await;
    let target_id = book_id_by_uuid(&pool, &target).await;

    // Full taxonomy on the source; a *shared* series on the target so the
    // insert hits an OR-IGNORE collision rather than a plain move.
    for (tbl, col, name) in [
        ("series", "name", "Gothic Horror"),
        ("tags", "name", "horror"),
        ("publishers", "name", "Archibald Constable"),
        ("languages", "code", "en"),
    ] {
        sqlx::query(&format!("INSERT INTO {tbl} ({col}) VALUES (?)"))
            .bind(name)
            .execute(&pool)
            .await
            .unwrap();
    }
    for (link, col, tbl, name_col, name, book) in [
        (
            "books_series_link",
            "series",
            "series",
            "name",
            "Gothic Horror",
            source_id,
        ),
        (
            "books_series_link",
            "series",
            "series",
            "name",
            "Gothic Horror",
            target_id,
        ),
        (
            "books_tags_link",
            "tag",
            "tags",
            "name",
            "horror",
            source_id,
        ),
        (
            "books_publishers_link",
            "publisher",
            "publishers",
            "name",
            "Archibald Constable",
            source_id,
        ),
        (
            "books_languages_link",
            "language",
            "languages",
            "code",
            "en",
            source_id,
        ),
    ] {
        sqlx::query(&format!(
            "INSERT INTO {link} (book, {col}) SELECT ?, id FROM {tbl} WHERE {name_col} = ?"
        ))
        .bind(book)
        .bind(name)
        .execute(&pool)
        .await
        .unwrap();
    }

    // Same-format audio progress on both (source newer -> its position wins),
    // plus bookmarks / sessions / highlights on both (no UNIQUE, blind move).
    for (uuid, pos, ts) in [(&target, 100.0, 1000), (&source, 200.0, 2000)] {
        sqlx::query(
            "INSERT INTO reading_progress (user_id, book_uuid, format, audio_position_seconds, updated_at)
             VALUES (?, ?, 'audio', ?, ?)",
        )
        .bind(user)
        .bind(uuid)
        .bind(pos)
        .bind(ts)
        .execute(&pool)
        .await
        .unwrap();
    }
    for uuid in [&source, &target] {
        sqlx::query("INSERT INTO bookmarks (user_id, book_uuid, position) VALUES (?, ?, 'x')")
            .bind(user)
            .bind(uuid)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO listening_sessions (user_id, book_uuid, started_at, ended_at, seconds_listened) VALUES (?, ?, 1, 2, 1)")
            .bind(user).bind(uuid).execute(&pool).await.unwrap();
        sqlx::query(
            "INSERT INTO highlights (user_id, book_uuid, epub_cfi_range) VALUES (?, ?, 'r')",
        )
        .bind(user)
        .bind(uuid)
        .execute(&pool)
        .await
        .unwrap();
    }

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    // Every taxonomy link now hangs off the target; none off the deleted source.
    for (link, col) in [
        ("books_series_link", "series"),
        ("books_tags_link", "tag"),
        ("books_publishers_link", "publisher"),
        ("books_languages_link", "language"),
    ] {
        let on_target: i64 = sqlx::query_scalar(&format!(
            "SELECT COUNT(*) FROM {link} WHERE book = ? AND {col} IS NOT NULL"
        ))
        .bind(target_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(
            on_target, 1,
            "{link} should have exactly one row on the target"
        );
    }
    // Source book row is gone, so no link can reference it.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);

    // Progress deduped to the newer (source) position, re-parented to target.
    let prog: Vec<(String, f64)> =
        sqlx::query_as("SELECT book_uuid, audio_position_seconds FROM reading_progress")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(prog, vec![(target.clone(), 200.0)]);
    // Bookmarks / sessions / highlights: both books' rows now on the target.
    for tbl in ["bookmarks", "listening_sessions", "highlights"] {
        let on_target: i64 =
            sqlx::query_scalar(&format!("SELECT COUNT(*) FROM {tbl} WHERE book_uuid = ?"))
                .bind(&target)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(
            on_target, 2,
            "{tbl} should re-parent both books' rows to the target"
        );
    }
}

#[tokio::test]
async fn merge_books_rejects_same_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let err = merge_books(&pool, &uuid, &uuid, None).await.unwrap_err();
    assert!(matches!(err, MergeError::SameBook));
}

#[tokio::test]
async fn merge_books_rejects_unknown_uuid() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let uuid = seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    let err = merge_books(&pool, "nope", &uuid, None).await.unwrap_err();
    assert!(matches!(err, MergeError::BookNotFound(u) if u == "nope"));
}

#[tokio::test]
async fn merge_books_allows_same_format_and_assigns_ordinals() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let a = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let b = seed_ebook(&pool, "B/Dracula 1992.epub", "Dracula 1992", "Bram Stoker").await;
    let out = merge_books(&pool, &a, &b, None).await.unwrap();
    assert_eq!(out.target_uuid, b);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM book_files").await, 2);
    // The target's original file keeps ordinal 0; the merged source file
    // gets ordinal 1 with the source's title as label.
    let rows: Vec<(i64, String)> = sqlx::query_as(
        "SELECT ordinal, COALESCE(label, '') FROM book_files WHERE book_id = (SELECT id FROM books) ORDER BY ordinal",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(rows, vec![(0, String::new()), (1, "Dracula".to_string())]);
}

#[tokio::test]
async fn merge_books_assigns_consecutive_ordinals_for_multi_file_move() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Build a two-file source book by chaining a merge: a -> b leaves b
    // holding two EPUB files (ordinals 0 and 1).
    let a = seed_ebook(&pool, "A/One.epub", "One", "Bram Stoker").await;
    let b = seed_ebook(&pool, "B/Two.epub", "Two", "Bram Stoker").await;
    merge_books(&pool, &a, &b, None).await.unwrap();

    // Now merge the two-file b into a single-file target c: both moved
    // files must land at consecutive ordinals after c's own file, in one
    // batched UPDATE per format.
    let c = seed_ebook(&pool, "C/Three.epub", "Three", "Bram Stoker").await;
    merge_books(&pool, &b, &c, None).await.unwrap();

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM book_files").await, 3);
    // c's original file keeps ordinal 0; the two moved files (ordered by
    // their prior ordinal, then filename) take 1 and 2. The file that had
    // no label gets the source book's title ("Two"); the one already
    // labelled "One" (from the first merge) keeps it.
    let rows: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT ordinal, filename, COALESCE(label, '') FROM book_files
          WHERE book_id = (SELECT id FROM books) ORDER BY ordinal",
    )
    .fetch_all(&pool)
    .await
    .unwrap();
    assert_eq!(
        rows,
        vec![
            (0, "Three".to_string(), String::new()),
            (1, "Two".to_string(), "Two".to_string()),
            (2, "One".to_string(), "One".to_string()),
        ]
    );
}

#[tokio::test]
async fn merge_books_moves_progress_with_latest_wins_on_collision() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    // M4B + MP3: passes the file-format collision check but both map to
    // the coarse 'audio' progress format.
    let target = seed_audiobook(&pool, "A/Dracula.m4b", "Dracula", "Bram Stoker").await;
    let mut mp3 = indexed_audiobook("B/Drakula mp3", "Drakula", Some("Bram Stoker"));
    mp3.format = "MP3".into();
    let source_scan_key = mp3.scan_key.clone();
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![mp3],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    // Identity is minted (F2) — read the durable uuid back by scan_key.
    let source = crate::test_support::uuid_by_scan_key(&pool, &source_scan_key).await;

    // User-data tables soft-ref the durable `books.uuid` (F1).
    for (book_uuid, pos, ts) in [(&target, 100.0, 1000), (&source, 200.0, 2000)] {
        sqlx::query(
            "INSERT INTO reading_progress (user_id, book_uuid, format, audio_position_seconds, updated_at)
             VALUES (?, ?, 'audio', ?, ?)",
        )
        .bind(user)
        .bind(book_uuid)
        .bind(pos)
        .bind(ts)
        .execute(&pool)
        .await
        .unwrap();
    }

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    // Exactly one row survives, re-parented to the target: the newer
    // (source's) position.
    let rows: Vec<(String, f64)> =
        sqlx::query_as("SELECT book_uuid, audio_position_seconds FROM reading_progress")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows, vec![(target.clone(), 200.0)]);
}

#[tokio::test]
async fn merge_books_moves_playback_rate_with_latest_wins_on_collision() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_audiobook(&pool, "A/Dracula.m4b", "Dracula", "Bram Stoker").await;
    let mut mp3 = indexed_audiobook("B/Drakula mp3", "Drakula", Some("Bram Stoker"));
    mp3.format = "MP3".into();
    let source_scan_key = mp3.scan_key.clone();
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![mp3],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let source = crate::test_support::uuid_by_scan_key(&pool, &source_scan_key).await;

    for (book_uuid, playback_rate, updated_at) in [(&target, 1.25, 1000), (&source, 1.75, 2000)] {
        sqlx::query(
            "INSERT INTO audiobook_playback_preferences
                (user_id, book_uuid, playback_rate, updated_at)
             VALUES (?, ?, ?, ?)",
        )
        .bind(user)
        .bind(book_uuid)
        .bind(playback_rate)
        .bind(updated_at)
        .execute(&pool)
        .await
        .unwrap();
    }

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let rows: Vec<(String, f64)> =
        sqlx::query_as("SELECT book_uuid, playback_rate FROM audiobook_playback_preferences")
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(rows, vec![(target.clone(), 1.75)]);
}

#[tokio::test]
async fn merge_books_merges_overrides_with_target_keys_winning() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    crate::metadata_overrides::upsert_metadata_overrides(
        &pool,
        &target,
        &MetadataOverrides {
            title: Some("Dracula (Annotated)".into()),
            ..Default::default()
        },
        false,
        user,
    )
    .await
    .unwrap();
    crate::metadata_overrides::upsert_metadata_overrides(
        &pool,
        &source,
        &MetadataOverrides {
            title: Some("Drakula (Hungarian)".into()),
            description: Some("From the source".into()),
            ..Default::default()
        },
        false,
        user,
    )
    .await
    .unwrap();

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let json: String =
        sqlx::query_scalar("SELECT overrides FROM metadata_overrides WHERE book_uuid = ?")
            .bind(&target)
            .fetch_one(&pool)
            .await
            .unwrap();
    let merged: MetadataOverrides = serde_json::from_str(&json).unwrap();
    // Target's key wins; source-only key fills in.
    assert_eq!(merged.title.as_deref(), Some("Dracula (Annotated)"));
    assert_eq!(merged.description.as_deref(), Some("From the source"));
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM metadata_overrides").await,
        1
    );
}

#[tokio::test]
async fn merge_books_adopts_source_cover_when_target_has_none() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let source_id = book_id_by_uuid(&pool, &source).await;
    sqlx::query("UPDATE books SET has_cover = 1 WHERE id = ?")
        .bind(source_id)
        .execute(&pool)
        .await
        .unwrap();

    merge_books(&pool, &source, &target, None).await.unwrap();

    let has_cover: i64 = sqlx::query_scalar("SELECT has_cover FROM books WHERE uuid = ?")
        .bind(&target)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(has_cover, 1);
}

#[tokio::test]
async fn chained_merge_repoints_earlier_merged_uuids_to_final_target() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let a = seed_audiobook(&pool, "X/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let b = seed_ebook(&pool, "Y/Dracula.epub", "Dracula", "Bram Stoker").await;
    // Auto-attach is title-exact, so a and b stayed separate. A -> B:
    merge_books(&pool, &a, &b, None).await.unwrap();
    // Then B -> C (a PDF-only book, so no format collision).
    let c = seed_ebook(
        &pool,
        "Z/Dracula Deluxe.pdf",
        "Dracula Deluxe",
        "Bram Stoker",
    )
    .await;
    merge_books(&pool, &b, &c, None).await.unwrap();

    let c_id = book_id_by_uuid(&pool, &c).await;
    // Both absorbed uuids now point at C.
    let targets: Vec<i64> = sqlx::query_scalar("SELECT DISTINCT book_id FROM merged_uuids")
        .fetch_all(&pool)
        .await
        .unwrap();
    assert_eq!(targets, vec![c_id]);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 2);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM book_files").await, 3);
}

#[tokio::test]
async fn reindex_diff_classifies_merged_source_file_as_unchanged() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    merge_books(&pool, &source, &target, None).await.unwrap();

    // The source file is still on disk with the same stat the indexer
    // recorded; the next audiobook reindex must classify it Unchanged.
    let ab = indexed_audiobook("B/Drakula.m4b", "Drakula", Some("Bram Stoker"));
    let db_rows = list_merged_rows_for_formats(&pool, "/audio", &["M4B", "M4A", "MP3"])
        .await
        .unwrap();
    let disk = vec![crate::ebook::StatEntry {
        filename: ab.group_path.clone(),
        scan_key: ab.scan_key.clone(),
        mtime_epoch: ab.max_mtime_epoch,
        size_bytes: ab.total_size_bytes,
        error: None,
    }];
    let diff = diff_library(&disk, &db_rows, std::path::Path::new("/audio"), true);
    // The merged row's identity in the diff is `merged_uuids.uuid` = the
    // source book's (now-deleted) uuid, which equals `source`.
    assert_eq!(diff.unchanged, vec![source]);
    assert!(diff.new.is_empty());
}

#[tokio::test]
async fn undo_merge_restores_source_book_and_moves_file_back() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let out = merge_books(&pool, &source, &target, None).await.unwrap();

    let restored = undo_merge(&pool, out.merge_log_id).await.unwrap();
    assert_eq!(restored, source);

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 2);
    let new_source_id = book_id_by_uuid(&pool, &source).await;
    // The M4B file row (and its part) came back to the restored book.
    let owner: i64 = sqlx::query_scalar("SELECT book_id FROM book_files WHERE format = 'M4B'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(owner, new_source_id);
    // Restored metadata and links.
    let (title, path): (String, String) =
        sqlx::query_as("SELECT title, path FROM books WHERE id = ?")
            .bind(new_source_id)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(title, "Drakula");
    assert_eq!(path, "B");
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM books_authors_link").await,
        2
    );
    // Guard cleared; log marked undone; FTS has both rows again.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 0);
    let undone: Option<i64> = sqlx::query_scalar("SELECT undone_at FROM merge_log")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(undone.is_some());
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books_fts").await, 2);
}

#[tokio::test]
async fn undo_merge_restores_scan_key_so_reindex_finds_no_duplicate() {
    // Regression: undo used to omit `books.scan_key` from the recreated row,
    // leaving it NULL. The next reindex then couldn't match the on-disk file
    // to the restored (scan_key-less) row and inserted a second copy — the
    // duplicate that destabilized the audiobook E2E suite.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let source_scan_key = crate::helpers::scan_key_for("B/Drakula.m4b");

    let out = merge_books(&pool, &source, &target, None).await.unwrap();
    undo_merge(&pool, out.merge_log_id).await.unwrap();

    // The recreated row carries the source's original scan_key back.
    let restored_scan_key: Option<String> =
        sqlx::query_scalar("SELECT scan_key FROM books WHERE uuid = ?")
            .bind(&source)
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(restored_scan_key.as_deref(), Some(source_scan_key.as_str()));

    // A reindex now matches the on-disk file to the restored native row and
    // classifies it Unchanged — no `new` entry, so no duplicate insert.
    let ab = indexed_audiobook("B/Drakula.m4b", "Drakula", Some("Bram Stoker"));
    let db_rows = list_indexed_rows_for_formats(&pool, "/audio", &["M4B", "M4A", "MP3"])
        .await
        .unwrap();
    let disk = vec![crate::ebook::StatEntry {
        filename: ab.group_path.clone(),
        scan_key: ab.scan_key.clone(),
        mtime_epoch: ab.max_mtime_epoch,
        size_bytes: ab.total_size_bytes,
        error: None,
    }];
    let diff = diff_library(&disk, &db_rows, std::path::Path::new("/audio"), true);
    assert!(
        diff.new.is_empty(),
        "restored book must not reindex as New (would duplicate): {:?}",
        diff.new
    );
    assert_eq!(diff.unchanged, vec![source]);
}

#[tokio::test]
async fn undo_merge_stamps_now_when_snapshot_timestamp_is_unparseable() {
    // A pre-0038 merge snapshot stored `timestamp` as an ISO string, which
    // `de_epoch_flexible` can't convert to an epoch (-> None). The recreate must
    // fall back to now rather than leaving the restored row's date-added NULL.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let out = merge_books(&pool, &source, &target, None).await.unwrap();

    // Rewrite the persisted snapshot to the legacy string form.
    let json: String = sqlx::query_scalar("SELECT source_metadata FROM merge_log WHERE id = ?")
        .bind(out.merge_log_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    let mut snap: serde_json::Value = serde_json::from_str(&json).unwrap();
    snap["timestamp"] = serde_json::json!("2024-01-02 03:04:05");
    sqlx::query("UPDATE merge_log SET source_metadata = ? WHERE id = ?")
        .bind(snap.to_string())
        .bind(out.merge_log_id)
        .execute(&pool)
        .await
        .unwrap();

    undo_merge(&pool, out.merge_log_id).await.unwrap();

    let ts: Option<i64> = sqlx::query_scalar("SELECT timestamp FROM books WHERE uuid = ?")
        .bind(&source)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert!(
        matches!(ts, Some(e) if e > 1_700_000_000),
        "restored timestamp should fall back to now, got {ts:?}"
    );
}

#[tokio::test]
async fn undo_merge_with_deleted_file_restores_fileless_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let out = merge_books(&pool, &source, &target, None).await.unwrap();

    // The moved file vanished from disk and a reindex dropped its row.
    sqlx::query("DELETE FROM book_files WHERE format = 'M4B'")
        .execute(&pool)
        .await
        .unwrap();

    undo_merge(&pool, out.merge_log_id).await.unwrap();

    // The restored source exists with zero files — a legal state.
    let new_source_id = book_id_by_uuid(&pool, &source).await;
    let files: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM book_files WHERE book_id = ?")
        .bind(new_source_id)
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(files, 0);
}

#[tokio::test]
async fn undo_merge_rejects_unknown_log_id() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let err = undo_merge(&pool, 999).await.unwrap_err();
    assert!(matches!(err, MergeError::LogNotFound));
}

#[tokio::test]
async fn undo_merge_rejects_double_undo() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let out = merge_books(&pool, &source, &target, None).await.unwrap();
    undo_merge(&pool, out.merge_log_id).await.unwrap();
    let err = undo_merge(&pool, out.merge_log_id).await.unwrap_err();
    assert!(matches!(err, MergeError::AlreadyUndone));
}

#[tokio::test]
async fn undo_merge_returns_snapshot_error_when_source_metadata_is_corrupt() {
    // `undo_merge` replays `merge_log.source_metadata` via
    // `serde_json::from_str`; a row whose snapshot JSON is corrupt (truncated
    // by a bad manual edit or a partial write) must surface as
    // `MergeError::Snapshot` — the `#[from] serde_json::Error` variant — not
    // as a panic or a `Db` error. Overwrite a real log entry's snapshot with
    // malformed JSON to drive the decode failure.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    let out = merge_books(&pool, &source, &target, None).await.unwrap();

    sqlx::query("UPDATE merge_log SET source_metadata = ? WHERE id = ?")
        .bind("{ this is not valid json")
        .bind(out.merge_log_id)
        .execute(&pool)
        .await
        .unwrap();

    let err = undo_merge(&pool, out.merge_log_id).await.unwrap_err();
    assert!(matches!(err, MergeError::Snapshot(_)), "got {err:?}");
    assert!(
        err.to_string()
            .starts_with("merge snapshot encode/decode failed"),
        "got {err}"
    );
}

#[tokio::test]
async fn merge_moves_highlights_to_target() {
    // F1 merge fix: highlights are now in the re-parent set, so a manual
    // merge no longer loses the source book's highlights.
    let pool = init_db("sqlite::memory:").await.unwrap();
    let user = seed_user(&pool).await;
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;

    crate::highlights::create_highlight(
        &pool,
        user,
        &omnibus_shared::CreateHighlight {
            client_id: None,
            book_uuid: source.clone(),
            epub_cfi_range: "epubcfi(/6/4!/4/2,/1:0,/1:100)".into(),
            color: omnibus_shared::HighlightColor::Blue,
            text: None,
        },
    )
    .await
    .unwrap();

    merge_books(&pool, &source, &target, Some(user))
        .await
        .unwrap();

    let on_target = crate::highlights::list_highlights(&pool, user, &target)
        .await
        .unwrap();
    assert_eq!(
        on_target.len(),
        1,
        "the source book's highlight must re-parent to the target on merge"
    );
}
