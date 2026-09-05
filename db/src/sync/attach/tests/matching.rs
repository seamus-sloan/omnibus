//! The match itself: title + author normalization (ampersands, last/first
//! name order), the ambiguity and ownership guards that skip a match rather
//! than clobber an existing attachment, and the known-uuid reattach that
//! bypasses title matching.

use super::{seed_audiobook, seed_ebook};
use crate::pool::init_db;
use crate::sync::{sync_audiobooks, sync_books, AudiobookSyncPlan, SyncPlan};
use crate::test_support::{count_rows as count, indexed, indexed_audiobook, CoversTempDir};

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
async fn attach_matches_ampersand_title_against_spelled_out_and() {
    // The two libraries routinely disagree on how the conjunction is written
    // — an EPUB's OPF title against an audiobook's album tag. Both spellings
    // must fold to the same key or the pair silently stays two books.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(
        &pool,
        "Quill/Mirth.epub",
        "A Tale of Mirth & Magic",
        "Ada Quill",
    )
    .await;
    seed_audiobook(
        &pool,
        "Quill/Mirth.m4b",
        "A Tale of Mirth and Magic",
        "Ada Quill",
    )
    .await;

    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 1);
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM book_files").await, 2);
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
async fn two_audiobooks_matching_one_ebook_in_one_plan_do_not_clobber() {
    // Two distinct M4B files for the same work land in a single New plan. The
    // first attaches as the ebook's M4B edition; the second must become its
    // own book, not overwrite the first's file row.
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(
        &pool,
        "Sanderson/Wind and Truth.epub",
        "Wind and Truth",
        "Brandon Sanderson",
    )
    .await;
    sync_audiobooks(
        &pool,
        "/audio",
        AudiobookSyncPlan {
            new_books: vec![
                indexed_audiobook(
                    "Sanderson/wt-a.m4b",
                    "Wind and Truth",
                    Some("Brandon Sanderson"),
                ),
                indexed_audiobook(
                    "Sanderson/wt-b.m4b",
                    "Wind and Truth",
                    Some("Brandon Sanderson"),
                ),
            ],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // One attaches to the ebook, the other stands alone → 2 books, and both
    // M4B files survive (no delete-then-replace).
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM books").await, 2);
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'M4B'"
        )
        .await,
        2
    );
    // Only the attached file records a ledger row; the standalone book is
    // native, so exactly one merged_uuids row exists.
    assert_eq!(count(&pool, "SELECT COUNT(*) FROM merged_uuids").await, 1);
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

#[tokio::test]
async fn attach_refuses_when_the_books_own_native_file_holds_the_slot() {
    // A book's native file carries no `merged_uuids` row, so a guard that
    // consults only the ledger cannot see it holding the slot. Two on-disk
    // copies of one ebook then trade the single (book, EPUB) slot on every
    // scan, restamping `books.last_modified` forever (#2320).
    let _covers = CoversTempDir::new("attach_native_slot_guard");
    let pool = init_db("sqlite::memory:").await.unwrap();
    seed_ebook(&pool, "Sanderson/yumi-a.epub", "Yumi", "Brandon Sanderson").await;
    let book_id: i64 = sqlx::query_scalar("SELECT id FROM books")
        .fetch_one(&pool)
        .await
        .unwrap();

    // A stale ledger row of the kind a library reorganization leaves behind,
    // naming a second on-disk copy of the same book.
    let scan_key_b = crate::helpers::scan_key_for("Sanderson/yumi-b.epub");
    sqlx::query(
        "INSERT INTO merged_uuids (uuid, book_id, format, library_path, scan_key)
         VALUES ('stale-ledger', ?, 'EPUB', '/ebooks', ?)",
    )
    .bind(book_id)
    .bind(&scan_key_b)
    .execute(&pool)
    .await
    .unwrap();

    // The second copy arrives as New and replays that ledger row.
    sync_books(
        &pool,
        "/ebooks",
        SyncPlan {
            new_books: vec![indexed(
                "Sanderson/yumi-b.epub",
                Some("Yumi"),
                &["Brandon Sanderson"],
                &[],
                None,
                None,
            )],
            ..Default::default()
        },
    )
    .await
    .unwrap();

    // Assert the slot's occupancy by count rather than reading one row back:
    // a regression here produces a *second* `book_files` row, and an unordered
    // `fetch_one` would pick between them arbitrarily — flaking, or masking the
    // very failure this test exists to catch.
    assert_eq!(
        count(
            &pool,
            &format!(
                "SELECT COUNT(*) FROM book_files WHERE book_id = {book_id} AND format = 'EPUB'"
            )
        )
        .await,
        1,
        "the slot holds exactly one file"
    );
    assert_eq!(
        count(
            &pool,
            &format!(
                "SELECT COUNT(*) FROM book_files
                  WHERE book_id = {book_id} AND format = 'EPUB' AND filename = 'yumi-a'"
            )
        )
        .await,
        1,
        "the native file keeps its own slot"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM book_files WHERE format = 'EPUB'"
        )
        .await,
        2,
        "the second copy became its own book's file"
    );
    assert_eq!(
        count(
            &pool,
            "SELECT COUNT(*) FROM merged_uuids WHERE uuid = 'stale-ledger'"
        )
        .await,
        0,
        "the stale ledger row is forgotten so it stops replaying"
    );
}
