//! The boot backfill's repair of ghosted multi-attach state: surviving
//! book data is preserved, multipart guards are replanted, resurrected
//! duplicates are dropped (singly and in a batch), and a same-scan-key book
//! in another library is spared.

use super::super::*;

use crate::test_support::count_rows as count;

struct GhostedRepairFixture {
    audio_path: String,
    target_id: i64,
    target_uuid: String,
    healthy_uuid: String,
}

async fn seed_ghosted_repair_fixture(pool: &SqlitePool) -> GhostedRepairFixture {
    let audio_root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("test_data")
        .join("audiobooks")
        .join("generated")
        .join("grace_hopper_series");
    let audio_path = audio_root.to_string_lossy().into_owned();
    let target_uuid = crate::test_support::seed_synced_ebook(
        pool,
        "Hopper/The Compiled Tales.epub",
        "The Compiled Tales",
        "Grace Hopper",
    )
    .await;
    let healthy_uuid = crate::test_support::seed_synced_ebook(
        pool,
        "Lovelace/Notes.epub",
        "Notes",
        "Ada Lovelace",
    )
    .await;
    crate::sync::sync_audiobooks(
        pool,
        "/healthy-audio",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![crate::test_support::indexed_audiobook(
                "Lovelace/Notes.m4b",
                "Notes",
                Some("Ada Lovelace"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    crate::indexer::reindex_audiobooks(pool, &audio_path)
        .await
        .unwrap();
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(pool)
        .await
        .unwrap();
    let audio_file_id: i64 =
        sqlx::query_scalar("SELECT id FROM book_files WHERE book_id = ? AND format = 'MP3'")
            .bind(target_id)
            .fetch_one(pool)
            .await
            .unwrap();
    let initial_parts: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM book_file_parts WHERE book_file_id = ?")
            .bind(audio_file_id)
            .fetch_one(pool)
            .await
            .unwrap();
    assert_eq!(initial_parts, 2, "fixture must begin as a two-part book");

    sqlx::query("DELETE FROM book_file_parts WHERE book_file_id = ? AND ordinal = 0")
        .bind(audio_file_id)
        .execute(pool)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('ghosted-chapter-1', ?, 'MP3', ?, 'the_compiled_tales/chapter01.mp3')",
    )
    .bind(target_id)
    .bind(&audio_path)
    .execute(pool)
    .await
    .unwrap();

    seed_repair_user_data(pool, &target_uuid).await;
    assert_eq!(ghosted_slot_counts(pool, target_id).await, (2, 1));

    GhostedRepairFixture {
        audio_path,
        target_id,
        target_uuid,
        healthy_uuid,
    }
}

async fn seed_repair_user_data(pool: &SqlitePool, target_uuid: &str) {
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, is_admin)
         VALUES (1, 'repair-user', 'hash', 1)",
    )
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO reading_progress
            (user_id, book_uuid, format, audio_position_seconds)
         VALUES (1, ?, 'audio', 321.5)",
    )
    .bind(target_uuid)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO user_ratings (user_id, book_uuid, half_stars)
         VALUES (1, ?, 9)",
    )
    .bind(target_uuid)
    .execute(pool)
    .await
    .unwrap();
    sqlx::query(
        "INSERT INTO journal_entries (user_id, book_uuid, body_md)
         VALUES (1, ?, 'Preserve this note')",
    )
    .bind(target_uuid)
    .execute(pool)
    .await
    .unwrap();
}

async fn ghosted_slot_counts(pool: &SqlitePool, target_id: i64) -> (i64, i64) {
    sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM merged_uuids
               WHERE book_id = ? AND format = 'MP3'),
             (SELECT COUNT(*) FROM book_files
               WHERE book_id = ? AND format = 'MP3')",
    )
    .bind(target_id)
    .bind(target_id)
    .fetch_one(pool)
    .await
    .unwrap()
}

async fn repair_user_data_counts(pool: &SqlitePool, target_uuid: &str) -> (i64, i64, i64) {
    sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM reading_progress WHERE book_uuid = ?),
             (SELECT COUNT(*) FROM user_ratings WHERE book_uuid = ?),
             (SELECT COUNT(*) FROM journal_entries WHERE book_uuid = ?)",
    )
    .bind(target_uuid)
    .bind(target_uuid)
    .bind(target_uuid)
    .fetch_one(pool)
    .await
    .unwrap()
}

#[tokio::test]
async fn boot_backfill_repairs_ghosted_multi_attach_and_preserves_surviving_book_data() {
    let _covers = crate::test_support::CoversTempDir::new("ghosted-multi-attach-repair");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let fixture = seed_ghosted_repair_fixture(&pool).await;

    run_boot_backfills(&pool).await.unwrap();
    let repaired_slot = ghosted_slot_counts(&pool, fixture.target_id).await;
    assert_eq!(
        repaired_slot,
        (0, 0),
        "repair must clear the corrupt attachment slot so disk files re-flow"
    );
    run_boot_backfills(&pool).await.unwrap();
    assert_eq!(
        ghosted_slot_counts(&pool, fixture.target_id).await,
        repaired_slot
    );
    let preserved_books: (i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM books WHERE uuid = ?),
             (SELECT COUNT(*) FROM book_files bf
               JOIN books b ON b.id = bf.book_id
              WHERE b.uuid = ?),
             (SELECT COUNT(*) FROM merged_uuids
               WHERE book_id = (SELECT id FROM books WHERE uuid = ?))",
    )
    .bind(&fixture.target_uuid)
    .bind(&fixture.healthy_uuid)
    .bind(&fixture.healthy_uuid)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(preserved_books, (1, 2, 1));
    assert_eq!(
        repair_user_data_counts(&pool, &fixture.target_uuid).await,
        (1, 1, 1)
    );

    crate::indexer::reindex_audiobooks(&pool, &fixture.audio_path)
        .await
        .unwrap();
    let rebuilt: (i64, i64, i64) = sqlx::query_as(
        "SELECT
             (SELECT COUNT(*) FROM books WHERE uuid = ?),
             (SELECT COUNT(*) FROM book_files
               WHERE book_id = ? AND format = 'MP3'),
             (SELECT COUNT(*) FROM book_file_parts p
               JOIN book_files bf ON bf.id = p.book_file_id
              WHERE bf.book_id = ? AND bf.format = 'MP3')",
    )
    .bind(&fixture.target_uuid)
    .bind(fixture.target_id)
    .bind(fixture.target_id)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(
        rebuilt,
        (1, 1, 2),
        "the surviving book identity must regain one two-part audio file"
    );
    assert_eq!(
        repair_user_data_counts(&pool, &fixture.target_uuid).await,
        (1, 1, 1)
    );
}

/// Forge a per-file guard and replay the file so it attaches as another
/// same-format part under `book_id` (a manual multi-part merge).
async fn attach_m4b_part(pool: &SqlitePool, book_id: i64, group_path: &str) {
    let ab =
        crate::test_support::indexed_audiobook(group_path, "Wind and Truth", Some("Sanderson"));
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES (?, ?, 'M4B', '/audio', ?)",
    )
    .bind(crate::helpers::stable_uuid("/audio", group_path))
    .bind(book_id)
    .bind(&ab.scan_key)
    .execute(pool)
    .await
    .unwrap();
    crate::sync::sync_audiobooks(
        pool,
        "/audio",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![ab],
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

/// Insert a resurrected standalone book for `group_path` (the bug's output):
/// a books row whose scan_key equals an already-attached part's file.
async fn resurrect_standalone(pool: &SqlitePool, group_path: &str) {
    crate::sync::sync_audiobooks(
        pool,
        "/audio",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![crate::test_support::indexed_audiobook(
                group_path,
                "Wind and Truth",
                Some("Sanderson"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();
}

#[tokio::test]
async fn boot_backfill_replants_multipart_guards_and_drops_resurrected_duplicates() {
    let _covers = crate::test_support::CoversTempDir::new("multipart-guard-repair");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid = crate::test_support::seed_synced_ebook(
        &pool,
        "Sanderson/wt.epub",
        "Wind and Truth",
        "Sanderson",
    )
    .await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    for gp in [
        "Sanderson/wt-1.m4b",
        "Sanderson/wt-2.m4b",
        "Sanderson/wt-3.m4b",
    ] {
        attach_m4b_part(&pool, target_id, gp).await;
    }

    // Corrupt to the pre-#1126 state: parts 2 & 3 lost their guards and
    // resurrected as standalone books; part 3's dupe accrued user data.
    sqlx::query(
        "DELETE FROM merged_uuids WHERE scan_key IN ('Sanderson/wt-2.m4b', 'Sanderson/wt-3.m4b')",
    )
    .execute(&pool)
    .await
    .unwrap();
    resurrect_standalone(&pool, "Sanderson/wt-2.m4b").await;
    resurrect_standalone(&pool, "Sanderson/wt-3.m4b").await;
    let dupe3_uuid: String =
        sqlx::query_scalar("SELECT uuid FROM books WHERE scan_key = 'Sanderson/wt-3.m4b'")
            .fetch_one(&pool)
            .await
            .unwrap();
    sqlx::query(
        "INSERT INTO users (id, username, password_hash, is_admin) VALUES (1, 'u', 'h', 1)",
    )
    .execute(&pool)
    .await
    .unwrap();
    sqlx::query("INSERT INTO user_ratings (user_id, book_uuid, half_stars) VALUES (1, ?, 8)")
        .bind(&dupe3_uuid)
        .execute(&pool)
        .await
        .unwrap();
    // Precondition: 1 target + 2 standalone dupes; only part 1 keeps its guard.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 3);
    assert_eq!(
        count(
            &pool,
            &format!("SELECT COUNT(*) FROM merged_uuids WHERE book_id = {target_id}")
        )
        .await,
        1
    );

    run_boot_backfills(&pool).await.unwrap();

    // Every attached part regains a guard; the no-user-data dupe (part 2) is
    // deleted, but the rated dupe (part 3) is left for a deliberate merge.
    assert_eq!(
        count(
            &pool,
            &format!("SELECT COUNT(*) FROM merged_uuids WHERE book_id = {target_id}")
        )
        .await,
        3
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM books WHERE scan_key = 'Sanderson/wt-2.m4b'"
        )
        .await,
        0,
        "the unrated resurrected duplicate is removed"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM books WHERE scan_key = 'Sanderson/wt-3.m4b'"
        )
        .await,
        1,
        "a dupe carrying user data is preserved for manual handling"
    );
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM user_ratings").await, 1);

    // Idempotent: a second run changes nothing.
    run_boot_backfills(&pool).await.unwrap();
    assert_eq!(
        count(
            &pool,
            &format!("SELECT COUNT(*) FROM merged_uuids WHERE book_id = {target_id}")
        )
        .await,
        3
    );

    // The three parts now classify Unchanged on a reindex — no resurrection.
    let disk: Vec<_> = [
        "Sanderson/wt-1.m4b",
        "Sanderson/wt-2.m4b",
        "Sanderson/wt-3.m4b",
    ]
    .iter()
    .map(|gp| {
        let ab = crate::test_support::indexed_audiobook(gp, "Wind and Truth", Some("Sanderson"));
        crate::ebook::StatEntry {
            filename: ab.group_path.clone(),
            scan_key: ab.scan_key.clone(),
            mtime_epoch: ab.max_mtime_epoch,
            size_bytes: ab.total_size_bytes,
            error: None,
        }
    })
    .collect();
    let db_rows =
        crate::books::list_merged_rows_for_formats(&pool, "/audio", &["M4B", "M4A", "MP3"])
            .await
            .unwrap();
    let diff = crate::indexer::diff_library(&disk, &db_rows, std::path::Path::new("/audio"), true);
    assert_eq!(diff.unchanged.len(), 3);
    assert!(diff.new.is_empty());
}

/// Drives the multi-placeholder `IN (?, ?, ?)` shape, not just the single-row case above.
#[tokio::test]
async fn boot_backfill_drops_multiple_resurrected_duplicates_in_one_batch() {
    let _covers = crate::test_support::CoversTempDir::new("multi-dupe-repair");
    let pool = init_db("sqlite::memory:").await.unwrap();
    let target_uuid = crate::test_support::seed_synced_ebook(
        &pool,
        "Sanderson/wt.epub",
        "Wind and Truth",
        "Sanderson",
    )
    .await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    let parts = [
        "Sanderson/wt-1.m4b",
        "Sanderson/wt-2.m4b",
        "Sanderson/wt-3.m4b",
        "Sanderson/wt-4.m4b",
    ];
    for gp in parts {
        attach_m4b_part(&pool, target_id, gp).await;
    }

    // Corrupt every non-primary part to the pre-#1126 state: all three lose
    // their guard and resurrect as standalone (unrated) books.
    sqlx::query(
        "DELETE FROM merged_uuids WHERE scan_key IN \
         ('Sanderson/wt-2.m4b', 'Sanderson/wt-3.m4b', 'Sanderson/wt-4.m4b')",
    )
    .execute(&pool)
    .await
    .unwrap();
    for gp in &parts[1..] {
        resurrect_standalone(&pool, gp).await;
    }
    // Precondition: 1 target + 3 standalone dupes.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 4);

    run_boot_backfills(&pool).await.unwrap();

    // All three unrated dupes are removed in the same batch, and the FTS
    // rows deleted alongside them leave no orphans.
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM books").await,
        1,
        "every unrated resurrected duplicate is removed"
    );
    assert_eq!(
        count(
            &pool,
            &format!("SELECT COUNT(*) FROM merged_uuids WHERE book_id = {target_id}")
        )
        .await,
        4
    );
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books_fts").await, 1);
}

#[tokio::test]
async fn boot_repair_spares_a_same_scan_key_book_in_another_library() {
    // scan_key is only unique per (library_id, scan_key). A healthy book in
    // library B must not be deleted just because library A attached a file at
    // the same *relative* path — the dupe match is scoped to the same root.
    let _covers = crate::test_support::CoversTempDir::new("multipart-guard-crosslib");
    let pool = init_db("sqlite::memory:").await.unwrap();

    // Library A ("/audio-a"): an ebook with an attached m4b at "Dup/p.m4b".
    let target_uuid =
        crate::test_support::seed_synced_ebook(&pool, "A/book.epub", "Book A", "Author A").await;
    let target_id: i64 = sqlx::query_scalar("SELECT id FROM books WHERE uuid = ?")
        .bind(&target_uuid)
        .fetch_one(&pool)
        .await
        .unwrap();
    let ab = crate::test_support::indexed_audiobook("Dup/p.m4b", "Book A", Some("Author A"));
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES (?, ?, 'M4B', '/audio-a', ?)",
    )
    .bind(crate::helpers::stable_uuid("/audio-a", "Dup/p.m4b"))
    .bind(target_id)
    .bind(&ab.scan_key)
    .execute(&pool)
    .await
    .unwrap();
    crate::sync::sync_audiobooks(
        &pool,
        "/audio-a",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![ab],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Library B ("/audio-b"): a healthy standalone audiobook at the same
    // relative path, different title/author, no user data.
    crate::sync::sync_audiobooks(
        &pool,
        "/audio-b",
        crate::sync::AudiobookSyncPlan {
            new_books: vec![crate::test_support::indexed_audiobook(
                "Dup/p.m4b",
                "Book B",
                Some("Author B"),
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    run_boot_backfills(&pool).await.unwrap();

    // The library-B book survives — its file lives under a different scan root.
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM books b JOIN scan_roots l ON l.id = b.library_id
              WHERE b.scan_key = 'Dup/p.m4b' AND l.path = '/audio-b'"
        )
        .await,
        1
    );
}
