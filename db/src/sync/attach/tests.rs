use crate::books::list_merged_rows_for_formats;
use crate::helpers::stable_uuid;
use crate::indexer::diff_library;
use crate::pool::init_db;
use crate::sync::{sync_audiobooks, sync_books, AudiobookSyncPlan, SyncPlan};
use crate::test_support::{count_rows as count, indexed, indexed_audiobook, CoversTempDir};

async fn seed_ebook(pool: &sqlx::SqlitePool, filename: &str, title: &str, author: &str) {
    crate::test_support::seed_synced_ebook(pool, filename, title, author).await;
}

async fn seed_audiobook(pool: &sqlx::SqlitePool, group_path: &str, title: &str, author: &str) {
    crate::test_support::seed_synced_audiobook(pool, group_path, title, author).await;
}

#[tokio::test]
async fn audiobook_attaches_to_existing_ebook_with_matching_title_and_author() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM book_files").await, 2);
    let (mu_book, mu_lib): (i64, String) =
        sqlx::query_as("SELECT book_id, library_path FROM merged_uuids WHERE format = 'M4B'")
            .fetch_one(&pool)
            .await
            .unwrap();
    let ebook_id: i64 = sqlx::query_scalar("SELECT id FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(mu_book, ebook_id);
    assert_eq!(mu_lib, "/audio");
    // The attached file row carries its own location override so HLS
    // resolves parts against the audio root, not the ebook library.
    let (bf_lib, bf_path): (Option<String>, Option<String>) =
        sqlx::query_as("SELECT library_path, path FROM book_files WHERE format = 'M4B'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(bf_lib.as_deref(), Some("/audio"));
    assert_eq!(bf_path.as_deref(), Some("Stoker"));
    // Parts and chapters landed under the attached file row.
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM book_file_parts").await,
        1
    );
    // Target metadata untouched: the ebook's title/author survive.
    let title: String = sqlx::query_scalar("SELECT title FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "Dracula");
}

#[tokio::test]
async fn ebook_attaches_to_existing_audiobook_with_matching_title_and_author() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM book_files").await, 2);
    let mu_lib: String =
        sqlx::query_scalar("SELECT library_path FROM merged_uuids WHERE format = 'EPUB'")
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(mu_lib, "/ebooks");
}

#[tokio::test]
async fn attach_matches_author_across_last_first_name_order() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Stoker, Bram").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
}

#[tokio::test]
async fn attach_skipped_when_target_already_has_the_format() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_audiobook(&pool, "A/Dracula.m4b", "Dracula", "Bram Stoker").await;
    // Second M4B of the same work (e.g. a different rip) stays separate.
    seed_audiobook(&pool, "B/Dracula.m4b", "Dracula", "Bram Stoker").await;
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 2);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 0);
}

#[tokio::test]
async fn attach_skipped_when_title_is_ambiguous_across_candidates() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    // Two same-format copies of the work = two candidate books.
    seed_ebook(&pool, "A/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_ebook(&pool, "B/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 3);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 0);
}

#[tokio::test]
async fn attach_skipped_when_book_has_no_author() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![indexed_audiobook("Stoker/Dracula.m4b", "Dracula", None)],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 2);
}

#[tokio::test]
async fn attach_skipped_when_titles_differ() {
    // The whole point of exact matching: Dune must not absorb Dune Messiah.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Herbert/Dune.epub", "Dune", "Frank Herbert").await;
    seed_audiobook(
        &pool,
        "Herbert/Dune Messiah.m4b",
        "Dune Messiah",
        "Frank Herbert",
    )
    .await;
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 2);
}

#[tokio::test]
async fn reindex_diff_classifies_attached_file_as_unchanged() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    // The next audiobook reindex feeds merged rows into the diff; a
    // matching on-disk stat must classify Unchanged, not New.
    let ab = indexed_audiobook("Stoker/Dracula.m4b", "Dracula", Some("Bram Stoker"));
    let db_rows = list_merged_rows_for_formats(&pool, "/audio", &["M4B", "M4A", "MP3"])
        .await
        .unwrap();
    let disk = vec![crate::ebook::StatEntry {
        filename: ab.group_path.clone(),
        uuid: ab.uuid.clone(),
        mtime_epoch: ab.max_mtime_epoch,
        size_bytes: ab.total_size_bytes,
        error: None,
    }];
    let diff = diff_library(&disk, &db_rows, std::path::Path::new("/audio"));
    assert_eq!(diff.unchanged, vec![ab.uuid.clone()]);
    assert!(diff.new.is_empty());
    assert!(diff.removed.is_empty());
}

#[tokio::test]
async fn changed_attached_file_refreshes_file_row_without_touching_target() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    // Re-sync the audiobook through the Changed bucket with a new stat
    // and a (suspicious) new title — the file row must refresh, the
    // target book's metadata must not.
    let mut ab = indexed_audiobook(
        "Stoker/Dracula.m4b",
        "Dracula Unabridged",
        Some("Bram Stoker"),
    );
    ab.total_size_bytes = 9999;
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            changed_books: vec![ab],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    let size: i64 = sqlx::query_scalar("SELECT size_bytes FROM book_files WHERE format = 'M4B'")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(size, 9999);
    let title: String = sqlx::query_scalar("SELECT title FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(title, "Dracula");
}

#[tokio::test]
async fn removed_attached_file_drops_attachment_but_keeps_book() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    let ab_uuid: String = sqlx::query_scalar("SELECT uuid FROM merged_uuids")
        .fetch_one(&pool)
        .await
        .unwrap();
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            removed_uuids: vec![ab_uuid],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 0);
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        0
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'EPUB'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM book_file_parts").await,
        0
    );
}

#[tokio::test]
async fn removed_ebook_with_attached_audiobook_cascades_and_audiobook_recreates() {
    // Deleting the target book's own file removes the books row (its
    // uuid IS the row); the attached file's merged_uuids entry cascades,
    // so the next audiobook scan recreates it standalone — self-healing.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    let ebook_uuid = stable_uuid("/ebooks", "Stoker/Dracula.epub");
    sync_books(
        &pool,
        "/ebooks",
        SyncPlan {
            removed_uuids: vec![ebook_uuid],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 0);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 0);

    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 0);
}

#[tokio::test]
async fn ebook_changed_reparse_preserves_attached_audiobook_file() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    sync_books(
        &pool,
        "/ebooks",
        SyncPlan {
            changed_books: vec![indexed(
                "Stoker/Dracula.epub",
                Some("Dracula (Annotated)"),
                &["Bram Stoker"],
                &[],
                None,
                None,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // The format-scoped wipe must leave the attached M4B row (and its
    // parts) alone.
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        1
    );
    assert_eq!(
        count(&pool, "SELECT COUNT(*) FROM book_file_parts").await,
        1
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'EPUB'"
        )
        .await,
        1
    );
}

#[tokio::test]
async fn attached_audiobook_cover_is_adopted_when_target_has_none() {
    let _covers = CoversTempDir::new("attach_adopt");
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;

    let mut ab = indexed_audiobook("Stoker/Dracula.m4b", "Dracula", Some("Bram Stoker"));
    ab.cover = Some(("image/jpeg".into(), vec![1, 2, 3]));
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![ab],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    let has_cover: i64 = sqlx::query_scalar("SELECT has_cover FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();
    assert_eq!(has_cover, 1);
}

#[tokio::test]
async fn known_uuid_reattaches_even_when_titles_no_longer_match() {
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Stoker/Dracula.epub", "Dracula", "Bram Stoker").await;
    seed_audiobook(&pool, "Stoker/Dracula.m4b", "Dracula", "Bram Stoker").await;

    // Simulate the self-healing path: the attachment row vanished but
    // merged_uuids still knows the file. A New sync with a *different*
    // title must still re-attach via the uuid.
    sqlx::query("DELETE FROM book_files WHERE format = 'M4B'")
        .execute(&pool)
        .await
        .unwrap();
    seed_audiobook(
        &pool,
        "Stoker/Dracula.m4b",
        "Dracula: Special Edition",
        "Bram Stoker",
    )
    .await;

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        1
    );
}
