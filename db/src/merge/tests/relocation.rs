//! The merge transaction itself: file, link and log relocation, taxonomy
//! and user-data transfer, same-format ordinals, override merging with the
//! target's keys winning, cover adoption, the physical-root promotion, and
//! the same-book / unknown-uuid / DB-failure rejections.

use omnibus_shared::MetadataOverrides;

use super::super::*;
use super::{book_id_by_uuid, seed_user};
use crate::pool::init_db;
use crate::sync::{sync_audiobooks, AudiobookSyncPlan};
use crate::test_support::{
    count_rows as count, indexed_audiobook, seed_synced_audiobook as seed_audiobook,
    seed_synced_ebook as seed_ebook,
};

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
            "INSERT INTO annotations (user_id, book_uuid, epub_cfi_range) VALUES (?, ?, 'r')",
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
    for tbl in ["bookmarks", "listening_sessions", "annotations"] {
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
async fn merge_books_propagates_db_error_when_pool_is_closed() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    let source = seed_audiobook(&pool, "B/Drakula.m4b", "Drakula", "Bram Stoker").await;
    pool.close().await;

    let err = merge_books(&pool, &source, &target, None)
        .await
        .unwrap_err();
    assert!(matches!(err, MergeError::Db(_)));
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

/// Merging a file-bearing book into a fileless check-in/wishlist target must
/// promote the target off the `physical://local` pseudo-root — otherwise the
/// surviving book holds real files but stays invisible to every path-scoped
/// read (All Books, search).
#[tokio::test]
async fn merge_books_promotes_a_physical_root_target_that_gains_files() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target = crate::physical::create_fileless_book(
        &pool,
        crate::physical::FilelessBook {
            title: "Paper Only".into(),
            authors: vec!["Print Author".into()],
            isbn: None,
            pubdate: None,
            description: None,
            cover: None,
        },
    )
    .await
    .unwrap();
    // Distinct title so seeding doesn't auto-attach to the target first.
    let source = seed_ebook(
        &pool,
        "Print/digital.epub",
        "Paper Only Digital",
        "Print Author",
    )
    .await;

    merge_books(&pool, &source, &target, None).await.unwrap();

    let library_path: String = sqlx::query_scalar(
        "SELECT l.path FROM books b JOIN scan_roots l ON l.id = b.library_id WHERE b.uuid = ?",
    )
    .bind(&target)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        library_path, "/ebooks",
        "merge must promote off the pseudo-root"
    );
}

#[test]
fn map_settings_error_returns_other_for_non_db_variants() {
    let validation = super::super::undo::map_settings_error(
        crate::settings::SettingsError::Validation("bad".into()),
    );
    assert!(
        matches!(&validation, MergeError::Other(msg) if msg.contains("bad")),
        "expected Other carrying the validation message, got {validation:?}"
    );

    let json_err = serde_json::from_str::<i32>("nope").unwrap_err();
    let json =
        super::super::undo::map_settings_error(crate::settings::SettingsError::Json(json_err));
    assert!(
        matches!(json, MergeError::Other(_)),
        "expected Other, got {json:?}"
    );
}
